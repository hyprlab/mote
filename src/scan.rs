//! Finding RDP and VNC servers on the local networks. Nothing here touches
//! a widget: the interface runs [`run`] as a command and hears back through
//! [`Event`]s. An RDP server is only ever asked for its certificate and a
//! VNC server only heard to greet, never signed in to, and the scan runs
//! only when asked for: it sends a connection to every address in range,
//! which a network's monitoring can notice.

mod network;
mod probe;

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;

use gtk::gio;
use gtk::gio::prelude::*;
use tokio::task::JoinSet;

pub(crate) use network::{Network, NetworkError};

use crate::connection::{Connection, DEFAULT_PORT, Protocol, VNC_PORT};
use crate::rdp::server::{Server, certificate_name};

/// RDP's own port and the one GNOME's Desktop Sharing moves to when Remote
/// Login has that; VNC's first four displays.
pub(crate) const PORTS: [u16; 6] = [
    DEFAULT_PORT,
    3390,
    VNC_PORT,
    VNC_PORT + 1,
    VNC_PORT + 2,
    VNC_PORT + 3,
];

/// What a server on `port` is taken to speak.
pub(crate) fn protocol(port: u16) -> Protocol {
    if (VNC_PORT..VNC_PORT + 100).contains(&port) {
        Protocol::Vnc
    } else {
        Protocol::Rdp
    }
}

/// Probes under way at once: most wait out their timeout on an address with
/// nothing there, so this sets how long a scan takes, about four seconds for
/// each network, while staying far from the limit of 1024 open files.
const AT_ONCE: usize = 192;

/// How long a name gets to resolve, either way.
const LOOKUP_FOR: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub(crate) enum Event {
    /// The networks about to be scanned; none when there is nothing to scan.
    Networks(Vec<Network>),
    /// How much of the scan is done, from 0 to 1.
    Progress(f64),
    Found(Box<Found>),
    Done,
}

/// A server that answered.
#[derive(Debug, Clone)]
pub(crate) struct Found {
    pub(crate) address: SocketAddrV4,
    /// A name that resolves back to the address, which outlasts a lease
    /// from DHCP, or else the address.
    pub(crate) host: String,
    /// What it calls itself where `host` has to be the address; often empty.
    pub(crate) name: String,
    pub(crate) server: Option<Server>,
    pub(crate) protocol: Protocol,
    /// A Mac, which greets as macOS Screen Sharing does.
    pub(crate) macos: bool,
    /// A connection reaches this already.
    pub(crate) saved: bool,
}

impl Found {
    /// `base` made a connection to this computer: its address first, which
    /// answered the scan, and the name that resolves to it as another
    /// address, for when a lease from DHCP moves it. Named as the scan
    /// shows it.
    pub(crate) fn connection(&self, base: Connection) -> Connection {
        let ip = self.address.ip().to_string();
        let resolves = self.host != ip;
        let name = if self.name.is_empty() && resolves {
            self.host.clone()
        } else {
            self.name.clone()
        };
        Connection {
            host: ip,
            port: self.address.port(),
            other_addresses: if resolves {
                vec![self.host.clone()]
            } else {
                Vec::new()
            },
            name,
            protocol: self.protocol,
            macos: self.macos,
            ..base
        }
    }
}

/// The longest a scan of `network` can take, with nothing there to answer.
pub(crate) fn longest_scan(network: Network) -> Duration {
    probe::CONNECT_FOR * (network.size() * PORTS.len()) as u32 / AT_ONCE as u32
}

/// Scan the networks this computer is on and those `named`, sending what
/// turns up to `events`. `saved` holds the host and port of each connection
/// there is, to mark what is already saved.
pub(crate) async fn run(saved: Vec<(String, u16)>, named: Vec<Network>, events: impl Fn(Event)) {
    let interfaces = network::interfaces();
    let networks = network::to_scan(&interfaces, &named);
    events(Event::Networks(networks.clone()));
    let saved = Arc::new(resolve(saved).await);
    let own: Vec<Ipv4Addr> = interfaces.iter().map(|i| i.address).collect();
    let targets = network::targets(&networks, &own, &PORTS);
    let total = targets.len();

    // Started only as others end: a /16 is 131,068 probes, too many to hold
    // in memory waiting for their turn.
    let mut probes = JoinSet::new();
    let mut done = 0;
    let finished = |result: Result<Option<Found>, _>, done: &mut usize| {
        *done += 1;
        events(Event::Progress(*done as f64 / total as f64));
        if let Ok(Some(found)) = result {
            events(Event::Found(Box::new(found)));
        }
    };
    for address in targets {
        if probes.len() >= AT_ONCE
            && let Some(result) = probes.join_next().await
        {
            finished(result, &mut done);
        }
        let saved = saved.clone();
        probes.spawn(async move {
            let answer = probe::probe(address).await?;
            Some(identify(address, answer, &saved).await)
        });
    }
    while let Some(result) = probes.join_next().await {
        finished(result, &mut done);
    }
    events(Event::Done);
}

/// The addresses the saved connections reach. A tunnel or a gateway's
/// connections aren't among them: their host is seen from the far end.
async fn resolve(saved: Vec<(String, u16)>) -> HashSet<SocketAddrV4> {
    let mut lookups = JoinSet::new();
    for (host, port) in saved {
        lookups.spawn(async move { addresses(&host, port).await });
    }
    let mut found = HashSet::new();
    while let Some(result) = lookups.join_next().await {
        found.extend(result.unwrap_or_default());
    }
    found
}

async fn addresses(host: &str, port: u16) -> Vec<SocketAddrV4> {
    let lookup = tokio::net::lookup_host((host, port));
    match tokio::time::timeout(LOOKUP_FOR, lookup).await {
        Ok(Ok(found)) => found
            .filter_map(|address| match address {
                SocketAddr::V4(address) => Some(address),
                SocketAddr::V6(_) => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

async fn identify(
    address: SocketAddrV4,
    answer: probe::Answer,
    saved: &HashSet<SocketAddrV4>,
) -> Found {
    let der = answer.certificate.as_deref();
    let reversed = reverse(*address.ip()).await;
    let resolves = match &reversed {
        Some(name) => addresses(name, address.port()).await.contains(&address),
        None => false,
    };
    let (host, name) = match reversed {
        Some(name) if resolves => (name, String::new()),
        name => {
            let name = name.or_else(|| {
                der.and_then(certificate_name)
                    .filter(|name| is_host_name(name))
            });
            (address.ip().to_string(), name.unwrap_or_default())
        }
    };
    Found {
        address,
        host,
        name,
        server: der.and_then(Server::from_certificate),
        protocol: protocol(address.port()),
        macos: answer.macos,
        saved: saved.contains(&address),
    }
}

/// The name DNS gives the address. GIO's resolver goes through the system's,
/// which blocks, so it gets a thread of the runtime's.
async fn reverse(ip: Ipv4Addr) -> Option<String> {
    let lookup = tokio::task::spawn_blocking(move || {
        gio::Resolver::default()
            .lookup_by_address(
                &gio::InetAddress::from(IpAddr::V4(ip)),
                gio::Cancellable::NONE,
            )
            .ok()
            .map(|name| name.to_string())
    });
    let name = tokio::time::timeout(LOOKUP_FOR, lookup)
        .await
        .ok()?
        .ok()??;
    let name = name.trim_end_matches('.');
    is_host_name(name).then(|| name.to_owned())
}

/// Whether `name` can stand for a computer. It comes from the network, and
/// systemd's resolver offers names like `_gateway` of its own.
fn is_host_name(name: &str) -> bool {
    name.len() <= 253
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        && !name.eq_ignore_ascii_case("localhost")
        && name.parse::<Ipv4Addr>().is_err()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wide_network_s_scan_is_told_in_minutes() {
        let seconds = |text: &str| longest_scan(text.parse().unwrap()).as_secs();
        assert_eq!(seconds("10.0.1.0/24"), 11);
        assert_eq!(seconds("10.0.0.0/20"), 191);
        assert_eq!(seconds("10.0.0.0/16"), 3071);
    }

    #[test]
    fn an_added_computer_is_reached_by_its_address_then_its_name() {
        let found = |host: &str, name: &str| Found {
            address: "10.0.1.17:5900".parse().unwrap(),
            host: host.into(),
            name: name.into(),
            server: None,
            protocol: Protocol::Vnc,
            macos: false,
            saved: false,
        };
        let c = found("mint.lan", "").connection(Connection::default());
        assert_eq!((c.host.as_str(), c.port), ("10.0.1.17", 5900));
        assert_eq!(c.other_addresses, ["mint.lan"]);
        assert_eq!((c.name.as_str(), c.protocol), ("mint.lan", Protocol::Vnc));
        // A name that doesn't resolve back can't be an address.
        let c = found("10.0.1.17", "mint").connection(Connection::default());
        assert_eq!(c.host, "10.0.1.17");
        assert!(c.other_addresses.is_empty());
        assert_eq!(c.name, "mint");
    }

    #[test]
    fn only_a_computer_s_name_counts_as_one() {
        for name in ["carbon", "dev-fedora.kepler.lan", "PC01"] {
            assert!(is_host_name(name), "{name}");
        }
        for name in [
            "",
            "_gateway",
            "localhost",
            "10.0.1.33",
            "a b",
            "-x",
            "<b>",
            "user@host",
        ] {
            assert!(!is_host_name(name), "{name}");
        }
    }
}
