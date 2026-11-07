//! What Mote keeps on disk: `settings.toml`, `connections.toml` and
//! `certificates.toml` in the directory `paths::config_dir` names, the same
//! for every kind of install. Passwords are never here; see `secrets`.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::connection::{Connection, DEFAULT_PORT, Quality, Scaling};
use crate::rdp::h264::Source;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ColorScheme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SortOrder {
    #[default]
    Name,
    RecentlyUsed,
    /// The order of `connections.toml`, which dragging rows sets.
    Custom,
}

impl SortOrder {
    pub(crate) const ALL: [Self; 3] = [Self::Name, Self::RecentlyUsed, Self::Custom];

    /// Its name in the `win.sort` action.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::RecentlyUsed => "recently-used",
            Self::Custom => "custom",
        }
    }

    pub(crate) fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|o| o.key() == key)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct WindowState {
    pub(crate) width: i32,
    pub(crate) height: i32,
    pub(crate) maximized: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            width: 1100,
            height: 720,
            maximized: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Settings {
    pub(crate) color_scheme: ColorScheme,
    pub(crate) sort: SortOrder,
    pub(crate) show_groups: bool,
    pub(crate) show_hosts: bool,
    /// Each connection in the sidebar shows its picture.
    pub(crate) show_avatars: bool,
    /// The chosen connection's page shows its picture; the editor always
    /// does, to choose it by.
    pub(crate) show_large_avatars: bool,
    /// What a new connection starts with.
    pub(crate) default_port: u16,
    pub(crate) default_username: String,
    pub(crate) default_domain: String,
    pub(crate) default_scaling: Scaling,
    pub(crate) default_quality: Quality,
    pub(crate) default_clipboard: bool,
    pub(crate) default_sound: bool,
    pub(crate) default_microphone: bool,
    pub(crate) default_fullscreen: bool,
    pub(crate) default_new_window: bool,
    /// The keyboard layout sessions tell the server about, as a Windows
    /// layout ID; 0 follows this computer's.
    pub(crate) keyboard_layout: u32,
    /// Sessions hold the desktop's shortcuts back while they have focus.
    pub(crate) grab_keyboard: bool,
    /// A session whose network drops connects again.
    pub(crate) reconnect: bool,
    /// A session says, the first time, that the shortcuts are held back.
    pub(crate) grab_notice: bool,
    /// A session says when a server sends no H.264, and how to fix it.
    pub(crate) h264_warning: bool,
    /// A session says when GNOME Desktop Sharing keeps its screen's size,
    /// and how to make it resize.
    pub(crate) size_notice: bool,
    /// A connection's first session that shares the clipboard says the
    /// server can read it.
    pub(crate) clipboard_notice: bool,
    /// The H.264 decoder to try first; none tries them in their own order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) decoder: Option<Source>,
    /// The networks Scan Network covers besides this computer's own, as
    /// text: one that no longer reads as a network is left out, not an error
    /// that would set aside all of this file.
    pub(crate) scan_networks: Vec<String>,
    /// The About window's height, as it was left.
    pub(crate) about_height: i32,
    pub(crate) window: WindowState,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            color_scheme: ColorScheme::System,
            sort: SortOrder::Name,
            show_groups: true,
            show_hosts: true,
            show_avatars: true,
            show_large_avatars: true,
            default_port: DEFAULT_PORT,
            default_username: String::new(),
            default_domain: String::new(),
            default_scaling: Scaling::default(),
            default_quality: Quality::default(),
            default_clipboard: false,
            default_sound: true,
            default_microphone: false,
            default_fullscreen: false,
            default_new_window: false,
            keyboard_layout: 0,
            grab_keyboard: true,
            reconnect: true,
            grab_notice: true,
            h264_warning: true,
            size_notice: true,
            clipboard_notice: true,
            decoder: None,
            scan_networks: Vec::new(),
            about_height: 740,
            window: WindowState::default(),
        }
    }
}

impl Settings {
    /// A blank connection carrying the defaults from Settings.
    pub(crate) fn new_connection(&self) -> Connection {
        Connection {
            port: self.default_port,
            username: self.default_username.clone(),
            domain: self.default_domain.clone(),
            scaling: self.default_scaling,
            quality: self.default_quality,
            clipboard: self.default_clipboard,
            sound: self.default_sound,
            microphone: self.default_microphone,
            fullscreen: self.default_fullscreen,
            new_window: self.default_new_window,
            ..Connection::default()
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct ConnectionsFile {
    #[serde(default, rename = "connection")]
    connections: Vec<Connection>,
}

fn dir() -> PathBuf {
    crate::paths::config_dir()
}

/// Add to the list at `to` what the one at `from` has and it lacks: the
/// connections by id, the trusted certificates by address. `false` for a
/// file that isn't one of these lists.
pub(crate) fn merge_list(from: &Path, to: &Path) -> bool {
    match from.file_name().and_then(|n| n.to_str()) {
        Some("connections.toml") => {
            let mut into = load::<ConnectionsFile>(to).unwrap_or_default();
            let extra = load::<ConnectionsFile>(from).unwrap_or_default();
            for connection in extra.connections {
                if !into.connections.iter().any(|c| c.id == connection.id) {
                    into.connections.push(connection);
                }
            }
            save(to, &into);
        }
        Some("certificates.toml") => {
            let mut into = load::<CertificatesFile>(to).unwrap_or_default();
            let extra = load::<CertificatesFile>(from).unwrap_or_default();
            for server in extra.servers {
                if !into
                    .servers
                    .iter()
                    .any(|s| s.port == server.port && s.host.eq_ignore_ascii_case(&server.host))
                {
                    into.servers.push(server);
                }
            }
            save(to, &into);
        }
        _ => return false,
    }
    true
}

pub(crate) fn load_settings() -> Settings {
    load(&dir().join("settings.toml")).unwrap_or_default()
}

pub(crate) fn save_settings(settings: &Settings) {
    save(&dir().join("settings.toml"), settings);
}

pub(crate) fn load_connections() -> Vec<Connection> {
    load::<ConnectionsFile>(&dir().join("connections.toml"))
        .unwrap_or_default()
        .connections
}

pub(crate) fn save_connections(connections: &[Connection]) {
    let file = ConnectionsFile {
        connections: connections.to_vec(),
    };
    save(&dir().join("connections.toml"), &file);
}

/// A server certificate trusted on first use, by the address it was
/// reached at; or a VNC server's RSA key, kept apart, as a server can have
/// both and each is trusted on its own.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TrustedServer {
    host: String,
    port: u16,
    fingerprint: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    key: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct CertificatesFile {
    #[serde(default, rename = "server")]
    servers: Vec<TrustedServer>,
}

fn certificates_path() -> PathBuf {
    dir().join("certificates.toml")
}

/// The fingerprint trusted for `host:port`, if any.
pub(crate) fn trusted_fingerprint(host: &str, port: u16, key: bool) -> Option<String> {
    load::<CertificatesFile>(&certificates_path())?
        .servers
        .into_iter()
        .find(|s| s.port == port && s.key == key && s.host.eq_ignore_ascii_case(host))
        .map(|s| s.fingerprint)
}

/// Trust `fingerprint` for `host:port` from now on, in place of any other.
pub(crate) fn trust_certificate(host: &str, port: u16, key: bool, fingerprint: &str) {
    let mut file = load::<CertificatesFile>(&certificates_path()).unwrap_or_default();
    file.servers
        .retain(|s| !(s.port == port && s.key == key && s.host.eq_ignore_ascii_case(host)));
    file.servers.push(TrustedServer {
        host: host.to_owned(),
        port,
        fingerprint: fingerprint.to_owned(),
        key,
    });
    save(&certificates_path(), &file);
}

/// Read a TOML file. One that doesn't parse (written by a newer version, or
/// edited by hand) is moved aside first, so the next save can't overwrite
/// what the user had.
fn load<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!("reading {}: {e}", path.display());
            return None;
        }
    };
    match toml::from_str(&text) {
        Ok(value) => Some(value),
        Err(e) => {
            let stamp = gtk::glib::DateTime::now_utc()
                .map(|t| t.to_unix())
                .unwrap_or_default();
            let aside = path.with_extension(format!("toml.unreadable-{stamp}"));
            tracing::warn!(
                "{} does not parse ({e}); kept as {}",
                path.display(),
                aside.display()
            );
            let _ = std::fs::rename(path, &aside);
            None
        }
    }
}

/// Write a TOML file whole, through a temporary file and a rename, so a
/// crash mid-write leaves the old one. Owner-only: host and user names are
/// nobody else's business.
fn save<T: Serialize>(path: &Path, value: &T) {
    let result = (|| -> std::io::Result<()> {
        let text = toml::to_string_pretty(value).map_err(std::io::Error::other)?;
        std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
        let tmp = path.with_extension("toml.tmp");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = result {
        tracing::error!("saving {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merging_lists_adds_only_what_is_missing() {
        let root = std::env::temp_dir().join(format!("mote-merge-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let (from, to) = (
            root.join("a/connections.toml"),
            root.join("connections.toml"),
        );
        let connection = |id: &str, host: &str| Connection {
            id: id.into(),
            host: host.into(),
            ..Connection::default()
        };
        save(
            &from,
            &ConnectionsFile {
                connections: vec![connection("1", "sandbox"), connection("2", "only-sandbox")],
            },
        );
        save(
            &to,
            &ConnectionsFile {
                connections: vec![connection("1", "host"), connection("3", "only-host")],
            },
        );
        assert!(merge_list(&from, &to));
        let hosts: Vec<String> = load::<ConnectionsFile>(&to)
            .unwrap()
            .connections
            .into_iter()
            .map(|c| c.host)
            .collect();
        assert_eq!(hosts, ["host", "only-host", "only-sandbox"]);
        assert!(!merge_list(&root.join("settings.toml"), &to));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn connections_round_trip() {
        let file = ConnectionsFile {
            connections: vec![Connection {
                name: "Office".into(),
                host: "box".into(),
                ..Connection::default()
            }],
        };
        let text = toml::to_string_pretty(&file).unwrap();
        let back: ConnectionsFile = toml::from_str(&text).unwrap();
        assert_eq!(back.connections, file.connections);
    }

    #[test]
    fn what_a_computer_runs_round_trips() {
        use crate::rdp::server::{Distro, Graphics, Server};
        let file = ConnectionsFile {
            connections: vec![Connection {
                host: "box".into(),
                server: Some(Server::GnomeLogin),
                distro: Some(Distro::Opensuse),
                graphics: Some(Graphics::Amd),
                ..Connection::default()
            }],
        };
        let text = toml::to_string_pretty(&file).unwrap();
        assert!(text.contains("server = \"gnome-remote-login\""), "{text}");
        let back: ConnectionsFile = toml::from_str(&text).unwrap();
        assert_eq!(back.connections, file.connections);
        // Unknown, it isn't written at all.
        let text = toml::to_string_pretty(&ConnectionsFile {
            connections: vec![Connection::default()],
        })
        .unwrap();
        assert!(!text.contains("distro"), "{text}");
    }

    #[test]
    fn notices_are_on_unless_turned_off() {
        let s: Settings = toml::from_str("reconnect = true").unwrap();
        assert!(s.grab_notice && s.h264_warning && s.size_notice);
        let s: Settings = toml::from_str("h264_warning = false").unwrap();
        assert!(s.grab_notice && !s.h264_warning && s.size_notice);
        let s: Settings = toml::from_str("size_notice = false").unwrap();
        assert!(s.grab_notice && s.h264_warning && !s.size_notice);
        assert!(s.clipboard_notice);
        let s: Settings = toml::from_str("clipboard_notice = false").unwrap();
        assert!(s.size_notice && !s.clipboard_notice);
    }

    #[test]
    fn missing_settings_take_defaults() {
        let s: Settings = toml::from_str("sort = \"recently-used\"").unwrap();
        assert_eq!(s.sort, SortOrder::RecentlyUsed);
        assert_eq!(s.default_port, DEFAULT_PORT);
        assert!(s.show_groups && s.show_avatars && s.show_large_avatars);
    }

    #[test]
    fn new_connections_keep_the_clipboard_to_themselves() {
        let c = Settings::default().new_connection();
        assert!(!c.clipboard && !c.clipboard_files);
        // A connection saved before sharing became opt-in keeps its choice.
        let c: Connection = toml::from_str("host = \"lab\"\nclipboard = true").unwrap();
        assert!(c.clipboard && !c.clipboard_files);
    }

    #[test]
    fn decoder_round_trips() {
        let mut s = Settings::default();
        assert!(!toml::to_string(&s).unwrap().contains("decoder"));
        s.decoder = Some(Source::CiscoOpenH264);
        let text = toml::to_string(&s).unwrap();
        assert!(text.contains("decoder = \"cisco-openh264\""));
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), s);
    }
}
