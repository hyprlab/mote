//! Remmina's connection profiles: one `.remmina` key file per connection,
//! in its data directory (or the Flatpak's). RDP and VNC profiles come
//! across; VNCI, which waits for a server to connect, doesn't.
//! Remmina keeps passwords encrypted with its own key or in the keyring
//! under its own schema, so they are not imported: the first connection
//! asks.

use std::path::PathBuf;

use crate::connection::{Connection, Network, Protocol, Quality, VncCodec, split_address};

/// Every directory Remmina may have kept profiles in.
fn dirs() -> Vec<PathBuf> {
    let home = gtk::glib::home_dir();
    vec![
        gtk::glib::user_data_dir().join("remmina"),
        home.join(".var/app/org.remmina.Remmina/data/remmina"),
        home.join(".remmina"),
    ]
}

/// The RDP and VNC connections in Remmina's profiles, and whether any profile
/// directory was found at all.
pub(crate) fn find() -> (Vec<Connection>, bool) {
    let mut found = Vec::new();
    let mut any_dir = false;
    for dir in dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        any_dir = true;
        let mut paths: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "remmina"))
            .collect();
        paths.sort();
        for path in paths {
            if let Ok(text) = std::fs::read_to_string(&path)
                && let Some(conn) = parse(&text)
            {
                found.push(conn);
            }
        }
    }
    (found, any_dir)
}

/// The connection in one profile, if it is an RDP or a VNC one.
pub(crate) fn parse(text: &str) -> Option<Connection> {
    let mut in_section = false;
    let mut conn = Connection::default();
    let mut protocol = None;
    let mut server = None;
    let mut quality = None;
    let mut loopback = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line == "[remmina]";
            continue;
        }
        if !in_section {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "protocol" => {
                protocol = match value.to_ascii_uppercase().as_str() {
                    "RDP" => Some(Protocol::Rdp),
                    "VNC" => Some(Protocol::Vnc),
                    _ => None,
                }
            }
            "name" => conn.name = value.to_owned(),
            "group" => conn.group = value.to_owned(),
            // Read once the protocol is known: VNC's take a display.
            "server" => server = Some(value.to_owned()),
            // VNC's: 0 is Poor, 1 Medium, 2 Good, 9 Best.
            "quality" => quality = Some(value.to_owned()),
            // VNC's, for a server without encryption.
            "disableencryption" => conn.allow_unencrypted = value == "1",
            "username" => conn.username = value.to_owned(),
            "domain" => conn.domain = value.to_owned(),
            "network" => conn.network = Network::from_remmina(value),
            // Remmina's color depth list holds its codec choice too.
            "colordepth" => {
                conn.quality = match value {
                    "65" => Quality::Avc420,
                    "0" | "63" | "64" => Quality::RemoteFx,
                    _ => Quality::Best,
                }
            }
            "disableclipboard" => conn.clipboard = value != "1",
            "sound" => conn.sound = value == "local",
            // Remmina keeps FreeRDP's device option there, such as sys:pulse.
            "microphone" => conn.microphone = !value.is_empty(),
            "sharefolder" => conn.folder = value.to_owned(),
            "viewonly" => conn.view_only = value == "1",
            // 1 is a window; 2, 3 and 4 are its kinds of fullscreen.
            "viewmode" => conn.fullscreen = matches!(value, "2" | "3" | "4"),
            "precommand" => conn.command_before = from_placeholders(value),
            "postcommand" => conn.command_after = from_placeholders(value),
            "ssh_tunnel_enabled" => conn.ssh_tunnel = value == "1",
            "ssh_tunnel_server" => conn.ssh_host = value.to_owned(),
            "ssh_tunnel_username" => conn.ssh_username = value.to_owned(),
            "ssh_tunnel_loopback" => loopback = value == "1",
            "gateway_server" => conn.gateway_host = value.to_owned(),
            "gateway_username" => conn.gateway_username = value.to_owned(),
            _ => {}
        }
    }
    let protocol = protocol?;
    let server = server.filter(|s| !s.trim().is_empty())?;
    match protocol {
        Protocol::Rdp => {
            let (host, port) = split_address(&server)?;
            conn.host = host;
            if let Some(port) = port {
                conn.port = port;
            }
        }
        Protocol::Vnc => {
            (conn.host, conn.port) = super::vnc::address(&server)?;
            conn.protocol = Protocol::Vnc;
            conn.vnc_codec = match quality.as_deref() {
                Some("0") => VncCodec::LowBandwidth,
                Some("9") => VncCodec::Lossless,
                _ => VncCodec::Best,
            };
        }
    }
    if conn.ssh_tunnel {
        // Remmina takes an empty SSH server to be the computer itself, and
        // "loopback" to mean the computer is the SSH server's own localhost.
        if conn.ssh_host.is_empty() {
            conn.ssh_host = conn.host.clone();
        }
        if loopback {
            conn.host = "localhost".to_owned();
        }
    }
    conn.gateway = !conn.gateway_host.is_empty();
    Some(conn.normalized())
}

/// Remmina pastes the connection into a command where it says `%h`, `%u`,
/// `%p` and `%g`; Mote hands it over in the environment.
fn from_placeholders(command: &str) -> String {
    command
        .replace("%h", "$MOTE_HOST")
        .replace("%u", "$MOTE_USER")
        .replace("%p", "$MOTE_NAME")
        .replace("%g", "$MOTE_GROUP")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_rdp_profile() {
        let text = "[remmina]\nname=Lab\nprotocol=RDP\nserver=lab.lan:3390\nusername=jo\ngroup=Work\npassword=.\n";
        let c = parse(text).unwrap();
        assert_eq!(
            (c.name.as_str(), c.host.as_str(), c.port),
            ("Lab", "lab.lan", 3390)
        );
        assert_eq!((c.username.as_str(), c.group.as_str()), ("jo", "Work"));
    }

    #[test]
    fn reads_the_network_codec_clipboard_and_sound() {
        let text = "[remmina]\nprotocol=RDP\nserver=lab\nnetwork=wan\ncolordepth=65\ndisableclipboard=1\nsound=remote\nmicrophone=sys:pulse\nsharefolder=/home/jo/Shared\n";
        let c = parse(text).unwrap();
        assert_eq!((c.network, c.quality), (Network::Wan, Quality::Avc420));
        assert!(!c.clipboard);
        assert!(!c.sound && c.microphone);
        assert_eq!(c.folder, "/home/jo/Shared");
    }

    #[test]
    fn reads_fullscreen() {
        let c = |mode: &str| {
            parse(&format!(
                "[remmina]\nprotocol=RDP\nserver=lab\nviewmode={mode}\n"
            ))
        };
        assert!(!c("1").unwrap().fullscreen);
        assert!(c("4").unwrap().fullscreen);
    }

    #[test]
    fn reads_the_tunnel_and_the_gateway() {
        let text = "[remmina]\nprotocol=RDP\nserver=box:3390\nssh_tunnel_enabled=1\nssh_tunnel_server=\nssh_tunnel_username=jo\nssh_tunnel_loopback=1\n";
        let c = parse(text).unwrap();
        assert_eq!(c.ssh_server(), Some(("box".into(), 22)));
        assert_eq!((c.host.as_str(), c.port), ("localhost", 3390));
        assert_eq!(c.ssh_username, "jo");
        let c = parse("[remmina]\nprotocol=RDP\nserver=box\ngateway_server=gw:8443\n").unwrap();
        assert_eq!(c.gateway_server(), Some(("gw".into(), 8443)));
    }

    #[test]
    fn reads_the_commands() {
        let text = "[remmina]\nprotocol=RDP\nserver=box\nprecommand=wol-wait %h\npostcommand=notify-send '%p closed'\n";
        let c = parse(text).unwrap();
        assert_eq!(c.command_before, "wol-wait $MOTE_HOST");
        assert_eq!(c.command_after, "notify-send '$MOTE_NAME closed'");
    }

    #[test]
    fn reads_a_vnc_profile_and_skips_other_protocols() {
        let c = parse("[remmina]\nprotocol=VNC\nserver=box:1\nquality=9\nviewonly=1\n").unwrap();
        assert_eq!(
            (c.protocol, c.host.as_str(), c.port),
            (Protocol::Vnc, "box", 5901)
        );
        assert_eq!(c.vnc_codec, VncCodec::Lossless);
        assert!(c.view_only);
        assert!(parse("[remmina]\nprotocol=VNCI\nserver=box\n").is_none());
        assert!(parse("[remmina]\nprotocol=SSH\nserver=box\n").is_none());
    }
}
