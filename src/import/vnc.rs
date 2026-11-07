//! `.vnc` files: TigerVNC's viewer writes `key=value` lines under its own
//! first line, RealVNC's an INI file with a `[Connection]` section; both
//! name the server as VNC addresses go, `host:display` or `host::port`.
//! Export writes TigerVNC's, which TigerVNC and RealVNC both read.

use crate::connection::{Connection, Protocol, VNC_PORT, split_address};

/// TigerVNC's first line, which its viewer looks for.
const TIGERVNC: &str = "TigerVNC Configuration file Version 1.0";

/// The host and port of a VNC address: `host::port` names the port,
/// `host:N` a display, which is port 5900 + N, when N is under 100, as
/// VNC's viewers read it; a larger number is taken as the port.
pub(crate) fn address(text: &str) -> Option<(String, u16)> {
    let text = text.trim();
    if let Some((host, port)) = text.rsplit_once("::")
        // Not an IPv6 address without brackets, which can't carry a port.
        && (!host.contains(':') || host.starts_with('['))
        && let Ok(port) = port.parse::<u16>()
        && port != 0
    {
        let (host, _) = split_address(host)?;
        return Some((host, port));
    }
    let (host, port) = split_address(text)?;
    let port = match port {
        Some(display) if display < 100 => VNC_PORT + display,
        Some(port) => port,
        None => VNC_PORT,
    };
    Some((host, port))
}

/// The connection in a `.vnc` file, named `name` unless the file says.
pub(crate) fn parse(text: &str, name: &str) -> Option<Connection> {
    let mut conn = Connection {
        name: name.to_owned(),
        protocol: Protocol::Vnc,
        port: VNC_PORT,
        ..Connection::default()
    };
    let mut found = false;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        let on = value == "1" || value.eq_ignore_ascii_case("true");
        match key.trim().to_ascii_lowercase().as_str() {
            // TigerVNC's, and RealVNC's.
            "servername" | "host" => {
                let (host, port) = address(value)?;
                (conn.host, conn.port) = (host, port);
                found = true;
            }
            "username" => conn.username = value.to_owned(),
            "viewonly" => conn.view_only = on,
            "fullscreen" => conn.fullscreen = on,
            _ => {}
        }
    }
    found.then(|| conn.normalized())
}

/// The connection as a TigerVNC `.vnc` file.
pub(crate) fn export(conn: &Connection) -> String {
    let host = if conn.host.contains(':') {
        format!("[{}]", conn.host)
    } else {
        conn.host.clone()
    };
    let mut lines = vec![
        TIGERVNC.to_owned(),
        String::new(),
        format!("ServerName={host}::{}", conn.port),
        format!("ViewOnly={}", u8::from(conn.view_only)),
        format!("FullScreen={}", u8::from(conn.fullscreen)),
    ];
    if !conn.username.is_empty() {
        lines.push(format!("Username={}", conn.username));
    }
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_displays_and_ports_as_vnc_viewers_do() {
        assert_eq!(address("box"), Some(("box".into(), 5900)));
        assert_eq!(address("box:1"), Some(("box".into(), 5901)));
        assert_eq!(address("box::5901"), Some(("box".into(), 5901)));
        assert_eq!(address("box:5999"), Some(("box".into(), 5999)));
        assert_eq!(address("[fd00::1]::5902"), Some(("fd00::1".into(), 5902)));
        // An IPv6 address without brackets carries no port. TigerVNC reads
        // `fd00::1` as `fd00` on port 1, and so does this.
        assert_eq!(address("fd00:1::2"), Some(("fd00:1::2".into(), 5900)));
        assert_eq!(address(""), None);
    }

    #[test]
    fn reads_what_tigervnc_and_realvnc_write() {
        let tiger = "TigerVNC Configuration file Version 1.0\n\nServerName=lab.lan:2\nViewOnly=1\n";
        let c = parse(tiger, "lab").unwrap();
        assert_eq!(
            (c.host.as_str(), c.port, c.protocol),
            ("lab.lan", 5902, Protocol::Vnc)
        );
        assert!(c.view_only);
        let real = "[Connection]\r\nHost=10.0.1.17\r\n[Options]\r\nFullScreen=1\r\n";
        let c = parse(real, "mint").unwrap();
        assert_eq!(
            (c.host.as_str(), c.port, c.name.as_str()),
            ("10.0.1.17", 5900, "mint")
        );
        assert!(c.fullscreen);
        assert!(parse("[Connection]\nNothing=here\n", "x").is_none());
    }

    #[test]
    fn export_reads_back() {
        let c = Connection {
            protocol: Protocol::Vnc,
            host: "lab.lan".into(),
            port: 5901,
            view_only: true,
            ..Connection::default()
        };
        let back = parse(&export(&c), "lab").unwrap();
        assert_eq!((back.host.as_str(), back.port), ("lab.lan", 5901));
        assert!(back.view_only);
    }
}
