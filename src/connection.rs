//! A saved connection: where it goes and who it signs in as. The password is
//! never part of it; that lives in the keyring (see `secrets`).

use serde::{Deserialize, Serialize};

use crate::rdp::server::{Distro, Graphics, Server};

/// The port RDP listens on unless told otherwise.
pub(crate) const DEFAULT_PORT: u16 = 3389;
/// The port of a VNC server's first display.
pub(crate) const VNC_PORT: u16 = 5900;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Connection {
    /// Stable across renames: the keyring item and the sidebar row find the
    /// connection by it.
    pub(crate) id: String,
    /// Empty means the host is the name.
    pub(crate) name: String,
    /// The seed of the robot it is shown as; empty for the one its id
    /// picks.
    pub(crate) robot: String,
    /// An image shown in place of the robot, by its name among the images
    /// kept (see `ui::avatar`); empty for none.
    pub(crate) picture: String,
    pub(crate) group: String,
    pub(crate) protocol: Protocol,
    /// A VNC connection to a Mac, which signs in with an account and is
    /// offered only the settings macOS takes. Kept beside the protocol
    /// rather than in it, so a version of Mote from before reads the
    /// connection as VNC instead of failing on the whole file.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) macos: bool,
    pub(crate) host: String,
    pub(crate) port: u16,
    /// More addresses of the same computer, as `host` or `host:port`, to
    /// try in turn when `host` can't be reached: its name on a VPN, say.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) other_addresses: Vec<String>,
    pub(crate) username: String,
    pub(crate) domain: String,
    /// Unix seconds of the last session, 0 for never.
    pub(crate) last_used: i64,
    pub(crate) scaling: Scaling,
    pub(crate) quality: Quality,
    /// VNC's codec choice, as `quality` is RDP's.
    pub(crate) vnc_codec: VncCodec,
    pub(crate) network: Network,
    /// Share the clipboard with the remote desktop.
    pub(crate) clipboard: bool,
    /// Offer files copied here to the remote desktop through the clipboard,
    /// which gives the server their paths and lets it read them.
    pub(crate) clipboard_files: bool,
    /// A session said that the server can read the clipboard; it says so
    /// at the first session that shares it.
    pub(crate) clipboard_told: bool,
    /// VNC: connect when the server offers no encryption, rather than stop
    /// before the password goes out.
    pub(crate) allow_unencrypted: bool,
    /// A session said how the server protects it, when that is less than
    /// encryption by a server whose identity is trusted; said once.
    pub(crate) protection_told: bool,
    /// Play the remote desktop's sound here, rather than leave it there.
    pub(crate) sound: bool,
    /// Send the microphone to the remote desktop.
    pub(crate) microphone: bool,
    /// A folder lent to the remote desktop; empty for none.
    pub(crate) folder: String,
    /// Show the desktop without sending it keys or the pointer.
    pub(crate) view_only: bool,
    /// A session to it opens fullscreen.
    pub(crate) fullscreen: bool,
    /// A session to it opens in a window of its own.
    pub(crate) new_window: bool,
    /// Run through `sh -c` before connecting; the connection waits for it,
    /// and doesn't go ahead when it fails. Empty for none.
    pub(crate) command_before: String,
    /// Run through `sh -c` once a session ends. Empty for none.
    pub(crate) command_after: String,
    /// Reach the computer through an SSH server.
    pub(crate) ssh_tunnel: bool,
    /// The SSH server, as `host` or `host:port`.
    pub(crate) ssh_host: String,
    /// The account on it; empty for the local user's name.
    pub(crate) ssh_username: String,
    /// Reach the computer through an RD Gateway.
    pub(crate) gateway: bool,
    /// The gateway, as `host` or `host:port`.
    pub(crate) gateway_host: String,
    /// The account it takes; empty for the connection's own.
    pub(crate) gateway_username: String,
    /// The computer's MAC address, to wake it with before connecting; empty
    /// for none.
    pub(crate) mac_address: String,
    /// What the computer runs, as last chosen in the advice on H.264; none
    /// until then.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) server: Option<Server>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) distro: Option<Distro>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) graphics: Option<Graphics>,
}

/// What the server speaks. Profiles from before VNC have none, and are RDP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Protocol {
    #[default]
    Rdp,
    Vnc,
}

impl Protocol {
    /// The port its servers listen on unless told otherwise.
    pub(crate) fn port(self) -> u16 {
        match self {
            Self::Rdp => DEFAULT_PORT,
            Self::Vnc => VNC_PORT,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Rdp => "RDP",
            Self::Vnc => "VNC",
        }
    }
}

/// What the editor's Protocol row offers: RDP, VNC, or VNC to a Mac.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Rdp,
    Vnc,
    Macos,
}

impl Kind {
    pub(crate) const ALL: [Self; 3] = [Self::Rdp, Self::Vnc, Self::Macos];

    pub(crate) fn protocol(self) -> Protocol {
        match self {
            Self::Rdp => Protocol::Rdp,
            Self::Vnc | Self::Macos => Protocol::Vnc,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Macos => "VNC (macOS)",
            other => other.protocol().name(),
        }
    }
}

/// How the remote desktop meets the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Scaling {
    /// The remote desktop takes the window's size and the screen's scale,
    /// and follows both.
    #[default]
    Resize,
    /// The remote desktop keeps its size; its picture is scaled to fit.
    Fit,
    /// One remote pixel to each pixel of the screen, scrolled when larger
    /// than the window.
    Actual,
}

impl Scaling {
    pub(crate) const ALL: [Self; 3] = [Self::Resize, Self::Fit, Self::Actual];

    /// The name an action state carries.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Resize => "resize",
            Self::Fit => "fit",
            Self::Actual => "actual",
        }
    }

    pub(crate) fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.key() == key)
    }
}

/// Which pictures the server may send. On these servers this is the
/// choice that matters: color depth applies only to the bitmaps of old,
/// which they don't send, and they ignore the visual effect flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Quality {
    /// H.264 in full color (AVC444) when the server has it, else as
    /// AVC420, else RemoteFX.
    #[default]
    Best,
    /// H.264 at half the color resolution (AVC420), never AVC444.
    Avc420,
    /// No H.264: RemoteFX and the pipeline's other codecs.
    RemoteFx,
}

impl Quality {
    pub(crate) const ALL: [Self; 3] = [Self::Best, Self::Avc420, Self::RemoteFx];
}

/// How a VNC server is asked to send its picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum VncCodec {
    /// H.264 where the server has it; else Tight, with JPEG at a high
    /// quality where the picture is a photo, and exact elsewhere.
    #[default]
    Best,
    /// Every pixel exact: ZRLE, and Tight without JPEG; no H.264.
    Lossless,
    /// For a slow network: Tight's JPEG at a low quality, compressed
    /// harder, and H.264 where the server has it.
    LowBandwidth,
}

impl VncCodec {
    pub(crate) const ALL: [Self; 3] = [Self::Best, Self::Lossless, Self::LowBandwidth];
}

/// The network the server is told it is on. xrdp sets its H.264 bitrate
/// by it; the others measure the network themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Network {
    #[default]
    Lan,
    Wan,
    BroadbandHigh,
    Satellite,
    BroadbandLow,
    Modem,
}

impl Network {
    pub(crate) const ALL: [Self; 6] = [
        Self::Lan,
        Self::Wan,
        Self::BroadbandHigh,
        Self::Satellite,
        Self::BroadbandLow,
        Self::Modem,
    ];

    /// The number `.rdp` files give it (`connection type:i:`); 7, detect
    /// automatically, is taken as the default, which is what xrdp does.
    pub(crate) fn from_number(number: u32) -> Self {
        match number {
            1 => Self::Modem,
            2 => Self::BroadbandLow,
            3 => Self::Satellite,
            4 => Self::BroadbandHigh,
            5 => Self::Wan,
            _ => Self::Lan,
        }
    }

    pub(crate) fn number(self) -> u32 {
        match self {
            Self::Modem => 1,
            Self::BroadbandLow => 2,
            Self::Satellite => 3,
            Self::BroadbandHigh => 4,
            Self::Wan => 5,
            Self::Lan => 6,
        }
    }

    /// Remmina's name for it (`network=`); `autodetect` and `none` are
    /// taken as the default.
    pub(crate) fn from_remmina(name: &str) -> Self {
        match name {
            "modem" => Self::Modem,
            "broadband-low" => Self::BroadbandLow,
            "satellite" => Self::Satellite,
            "broadband-high" => Self::BroadbandHigh,
            "wan" => Self::Wan,
            _ => Self::Lan,
        }
    }
}

impl Default for Connection {
    fn default() -> Self {
        Self {
            id: new_id(),
            name: String::new(),
            robot: String::new(),
            picture: String::new(),
            protocol: Protocol::Rdp,
            macos: false,
            group: String::new(),
            host: String::new(),
            port: DEFAULT_PORT,
            other_addresses: Vec::new(),
            username: String::new(),
            domain: String::new(),
            last_used: 0,
            scaling: Scaling::default(),
            quality: Quality::default(),
            vnc_codec: VncCodec::Best,
            network: Network::default(),
            // The server reads the local clipboard whenever it likes, so it
            // is shared only when asked for.
            clipboard: false,
            clipboard_files: false,
            clipboard_told: false,
            allow_unencrypted: false,
            protection_told: false,
            sound: true,
            microphone: false,
            folder: String::new(),
            view_only: false,
            fullscreen: false,
            new_window: false,
            command_before: String::new(),
            command_after: String::new(),
            ssh_tunnel: false,
            ssh_host: String::new(),
            ssh_username: String::new(),
            gateway: false,
            gateway_host: String::new(),
            gateway_username: String::new(),
            mac_address: String::new(),
            server: None,
            distro: None,
            graphics: None,
        }
    }
}

pub(crate) fn new_id() -> String {
    gtk::glib::uuid_string_random().to_string()
}

impl Connection {
    /// The folder to lend, if there is one still there to lend.
    pub(crate) fn shared_folder(&self) -> Option<std::path::PathBuf> {
        let folder = std::path::PathBuf::from(&self.folder);
        (!self.folder.is_empty() && folder.is_dir()).then_some(folder)
    }

    /// The SSH server to go through, and its port.
    pub(crate) fn ssh_server(&self) -> Option<(String, u16)> {
        let (host, port) = split_address(&self.ssh_host).filter(|_| self.ssh_tunnel)?;
        Some((host, port.unwrap_or(SSH_PORT)))
    }

    /// The RD Gateway to go through, and its port.
    pub(crate) fn gateway_server(&self) -> Option<(String, u16)> {
        // An RD Gateway leads to RDP alone; a VNC connection keeps the
        // setting only for a change back.
        let used = self.gateway && self.protocol == Protocol::Rdp;
        let (host, port) = split_address(&self.gateway_host).filter(|_| used)?;
        Some((host, port.unwrap_or(HTTPS_PORT)))
    }

    /// The MAC address to wake, if one is set and reads as one.
    pub(crate) fn wake_address(&self) -> Option<[u8; 6]> {
        parse_mac(&self.mac_address)
    }

    /// What the connection is called on screen.
    pub(crate) fn title(&self) -> &str {
        if self.name.trim().is_empty() {
            &self.host
        } else {
            &self.name
        }
    }

    /// Whether signing in takes a user name: VNC Authentication takes a
    /// password alone, and a Mac an account.
    pub(crate) fn takes_username(&self) -> bool {
        self.kind() != Kind::Vnc
    }

    pub(crate) fn kind(&self) -> Kind {
        match self.protocol {
            Protocol::Rdp => Kind::Rdp,
            Protocol::Vnc if self.macos => Kind::Macos,
            Protocol::Vnc => Kind::Vnc,
        }
    }

    /// The host, with the port only when it isn't the protocol's own.
    pub(crate) fn address(&self) -> String {
        if self.port == self.protocol.port() {
            self.host.clone()
        } else {
            with_port(&self.host, self.port)
        }
    }

    /// Everywhere the computer may be, in the order to try: the host, then
    /// the other addresses, which take the connection's port unless they
    /// name their own.
    pub(crate) fn addresses(&self) -> Vec<(String, u16)> {
        let others = self
            .other_addresses
            .iter()
            .filter_map(|other| split_address(other))
            .map(|(host, port)| (host, port.unwrap_or(self.port)));
        std::iter::once((self.host.clone(), self.port))
            .chain(others)
            .collect()
    }

    /// The account as Windows writes it, `DOMAIN\user`, or `None` when the
    /// connection names no user.
    pub(crate) fn account(&self) -> Option<String> {
        match (self.username.is_empty(), self.domain.is_empty()) {
            (true, _) => None,
            (false, true) => Some(self.username.clone()),
            (false, false) => Some(format!("{}\\{}", self.domain, self.username)),
        }
    }

    /// Whether a search for `query` (already lower case) finds this.
    pub(crate) fn matches(&self, query: &str) -> bool {
        [&self.name, &self.host, &self.username, &self.group]
            .into_iter()
            .chain(&self.other_addresses)
            .any(|f| f.to_lowercase().contains(query))
    }

    /// Whether `other` reaches the same account on the same machine, which
    /// is what makes an import a duplicate.
    pub(crate) fn same_target(&self, other: &Connection) -> bool {
        let theirs = other.addresses();
        self.username.eq_ignore_ascii_case(&other.username)
            && self
                .addresses()
                .iter()
                .any(|address| theirs.iter().any(|their| same_address(address, their)))
    }

    /// Fields as they should be stored: trimmed, a port typed into the host
    /// field moved to the port, and `DOMAIN\user` split.
    pub(crate) fn normalized(mut self) -> Self {
        self.name = self.name.trim().to_owned();
        self.group = self.group.trim().to_owned();
        self.domain = self.domain.trim().to_owned();
        self.username = self.username.trim().to_owned();
        for field in [
            &mut self.ssh_host,
            &mut self.ssh_username,
            &mut self.gateway_host,
            &mut self.gateway_username,
            &mut self.mac_address,
        ] {
            *field = field.trim().to_owned();
        }
        if let Some((host, port)) = split_address(&self.host) {
            self.host = host;
            if let Some(port) = port {
                self.port = port;
            }
        } else {
            self.host = self.host.trim().to_owned();
        }
        for other in std::mem::take(&mut self.other_addresses) {
            let Some((host, port)) = split_address(&other) else {
                continue;
            };
            let address = (host, port.unwrap_or(self.port));
            if !self.addresses().iter().any(|a| same_address(a, &address)) {
                let (host, _) = address;
                self.other_addresses.push(match port {
                    Some(port) => with_port(&host, port),
                    None => host,
                });
            }
        }
        if self.domain.is_empty()
            && let Some((domain, user)) = self.username.split_once('\\')
        {
            self.domain = domain.to_owned();
            self.username = user.to_owned();
        }
        self
    }
}

const SSH_PORT: u16 = 22;
const HTTPS_PORT: u16 = 443;

/// A host and port as people read them, the port only when it isn't RDP's
/// own.
pub(crate) fn address_text(host: &str, port: u16) -> String {
    if port == DEFAULT_PORT {
        host.to_owned()
    } else {
        with_port(host, port)
    }
}

fn with_port(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn same_address(a: &(String, u16), b: &(String, u16)) -> bool {
    a.1 == b.1 && a.0.eq_ignore_ascii_case(&b.0)
}

/// A MAC address as people write it: six pairs of hex digits, split by
/// colons or dashes, or not split at all.
pub(crate) fn parse_mac(text: &str) -> Option<[u8; 6]> {
    let digits: String = text
        .trim()
        .chars()
        .filter(|c| !matches!(c, ':' | '-'))
        .collect();
    if digits.len() != 12 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut mac = [0; 6];
    for (i, byte) in mac.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&digits[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(mac)
}

/// Split what someone typed as an address into a host and, if one was
/// given, a port: `host`, `host:port`, `[v6]:port`, a bare IPv6 address, or
/// any of those after `rdp://` or `vnc://`. `None` when it isn't an
/// address at all.
pub(crate) fn split_address(text: &str) -> Option<(String, Option<u16>)> {
    let text = text.trim();
    let text = text
        .strip_prefix("rdp://")
        .or_else(|| text.strip_prefix("vnc://"))
        .unwrap_or(text);
    let text = text.trim_end_matches('/');
    if text.is_empty() || text.chars().any(char::is_whitespace) {
        return None;
    }
    if let Some(rest) = text.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = match after.strip_prefix(':') {
            Some(p) => Some(p.parse().ok().filter(|&p| p != 0)?),
            None if after.is_empty() => None,
            None => return None,
        };
        return (!host.is_empty()).then(|| (host.to_owned(), port));
    }
    match text.matches(':').count() {
        1 => {
            let (host, port) = text.split_once(':')?;
            let port = port.parse().ok().filter(|&p| p != 0)?;
            (!host.is_empty()).then(|| (host.to_owned(), Some(port)))
        }
        // None, or several: an IPv6 address written without brackets, which
        // can't carry a port.
        _ => Some((text.to_owned(), None)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Mac is VNC with a flag beside it, so a Mote from before macOS
    /// connections reads it as VNC rather than failing on the whole file.
    #[test]
    fn a_mac_is_stored_as_vnc_with_a_flag() {
        let mut c = Connection {
            protocol: Protocol::Vnc,
            macos: true,
            ..Default::default()
        };
        let text = toml::to_string(&c).unwrap();
        assert!(text.contains("protocol = \"vnc\"") && text.contains("macos = true"));
        let read: Connection = toml::from_str(&text).unwrap();
        assert_eq!(read.kind(), Kind::Macos);
        assert!(read.takes_username());
        c.macos = false;
        let text = toml::to_string(&c).unwrap();
        assert!(!text.contains("macos"));
        assert_eq!(
            toml::from_str::<Connection>(&text).unwrap().kind(),
            Kind::Vnc
        );
    }

    #[test]
    fn addresses_split_into_host_and_port() {
        let s = |t| split_address(t);
        assert_eq!(s("box"), Some(("box".into(), None)));
        assert_eq!(s(" box.lan:3390 "), Some(("box.lan".into(), Some(3390))));
        assert_eq!(
            s("rdp://10.0.0.5:4000/"),
            Some(("10.0.0.5".into(), Some(4000)))
        );
        assert_eq!(s("[fe80::1]:3390"), Some(("fe80::1".into(), Some(3390))));
        assert_eq!(s("[fe80::1]"), Some(("fe80::1".into(), None)));
        assert_eq!(s("fe80::1"), Some(("fe80::1".into(), None)));
        assert_eq!(s("box:http"), None);
        assert_eq!(s("box:0"), None);
        assert_eq!(s("two words"), None);
        assert_eq!(s(""), None);
    }

    #[test]
    fn normalizing_moves_port_and_domain() {
        let c = Connection {
            host: " box:3390 ".into(),
            username: "CORP\\jo".into(),
            ..Connection::default()
        }
        .normalized();
        assert_eq!((c.host.as_str(), c.port), ("box", 3390));
        assert_eq!((c.domain.as_str(), c.username.as_str()), ("CORP", "jo"));
        assert_eq!(c.account().as_deref(), Some("CORP\\jo"));
    }

    #[test]
    fn other_addresses_follow_the_host_in_order() {
        let c = Connection {
            host: "box.lan".into(),
            port: 3390,
            other_addresses: vec!["box.vpn".into(), "[fd00::1]:4000".into()],
            ..Connection::default()
        };
        assert_eq!(
            c.addresses(),
            [
                ("box.lan".to_owned(), 3390),
                ("box.vpn".to_owned(), 3390),
                ("fd00::1".to_owned(), 4000),
            ]
        );
    }

    #[test]
    fn normalizing_tidies_other_addresses() {
        let c = Connection {
            host: "box".into(),
            port: 3390,
            other_addresses: vec![
                "".into(),
                " box.vpn ".into(),
                "rdp://[fd00::1]:3389/".into(),
                // Where it already goes, by another spelling.
                "BOX".into(),
                "box.vpn:3390".into(),
                "two words".into(),
            ],
            ..Connection::default()
        }
        .normalized();
        assert_eq!(c.other_addresses, ["box.vpn", "[fd00::1]:3389"]);
    }

    #[test]
    fn other_addresses_are_found_and_make_duplicates() {
        let c = Connection {
            host: "box.lan".into(),
            other_addresses: vec!["box.vpn".into()],
            ..Connection::default()
        };
        assert!(c.matches("vpn"));
        let imported = Connection {
            host: "BOX.VPN".into(),
            ..Connection::default()
        };
        assert!(c.same_target(&imported));
        assert!(imported.same_target(&c));
        let other_port = Connection {
            port: 3390,
            ..imported
        };
        assert!(!c.same_target(&other_port));
    }

    #[test]
    fn mac_addresses_parse_however_written() {
        let mac = Some([0x52, 0x54, 0x00, 0xab, 0xcd, 0xef]);
        assert_eq!(parse_mac("52:54:00:ab:cd:ef"), mac);
        assert_eq!(parse_mac(" 52-54-00-AB-CD-EF "), mac);
        assert_eq!(parse_mac("525400abcdef"), mac);
        assert_eq!(parse_mac("52:54:00:ab:cd"), None);
        assert_eq!(parse_mac("52:54:00:ab:cd:eg"), None);
        assert_eq!(parse_mac(""), None);
    }

    #[test]
    fn tunnels_take_their_usual_ports() {
        let c = Connection {
            ssh_tunnel: true,
            ssh_host: "bastion".into(),
            gateway_host: "gw.example.com:8443".into(),
            ..Connection::default()
        };
        assert_eq!(c.ssh_server(), Some(("bastion".into(), 22)));
        // Set, but turned off.
        assert_eq!(c.gateway_server(), None);
        let c = Connection { gateway: true, ..c };
        assert_eq!(c.gateway_server(), Some(("gw.example.com".into(), 8443)));
    }

    #[test]
    fn address_shows_port_only_when_unusual() {
        let mut c = Connection {
            host: "box".into(),
            ..Connection::default()
        };
        assert_eq!(c.address(), "box");
        c.port = 3390;
        assert_eq!(c.address(), "box:3390");
        c.host = "fe80::1".into();
        assert_eq!(c.address(), "[fe80::1]:3390");
    }
}
