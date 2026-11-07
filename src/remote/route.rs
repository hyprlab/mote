//! The way to the server: straight there over TCP, through an SSH tunnel,
//! or through an RD Gateway (RDP's own, in `rdp::gateway`), after waking
//! the computer when asked to.

mod ssh;
mod wake;

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use super::{Event, Failure, Target, Unreachable, Via};

/// A connection to the server, whichever way it goes.
pub(crate) trait Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + Sync> Stream for T {}

const TCP_TIMEOUT: Duration = Duration::from_secs(15);

/// How long a computer woken over the network gets to come up.
const WAKE_FOR: Duration = Duration::from_secs(120);

/// How often the packet goes out again while it does: one can be lost,
/// and a computer only part way asleep can miss it.
const WAKE_AGAIN: Duration = Duration::from_secs(15);

/// How long each try waits for an answer meanwhile: a computer asleep
/// never answers, so a short wait only tells sooner that it still sleeps.
const WAKE_TRY: Duration = Duration::from_secs(3);

/// How long an address gets when another is there to try: going on to the
/// VPN shouldn't take as long as giving up on the computer altogether.
const ANOTHER_ADDRESS_AFTER: Duration = Duration::from_secs(5);

/// How long to wait for an address to answer.
pub(crate) fn patience(waking: bool, last: bool) -> Duration {
    if waking {
        WAKE_TRY
    } else if last {
        TCP_TIMEOUT
    } else {
        ANOTHER_ADDRESS_AFTER
    }
}

/// A computer being woken over the network: the packets sent to it, and how
/// long it has left to answer.
pub(crate) struct Waking {
    mac: [u8; 6],
    until: Instant,
    sent: Option<Instant>,
    told: bool,
}

impl Waking {
    pub(crate) fn new(mac: [u8; 6]) -> Self {
        Self {
            mac,
            until: Instant::now() + WAKE_FOR,
            sent: None,
            told: false,
        }
    }

    /// Send the packet, if it is time to.
    pub(crate) async fn send(&mut self) {
        if self.sent.is_none_or(|at| at.elapsed() >= WAKE_AGAIN) {
            wake::send(self.mac).await;
            self.sent = Some(Instant::now());
        }
    }

    /// Wait before trying again; `false` when the computer's time is up.
    pub(crate) async fn wait(&mut self, host: &str, events: &mpsc::Sender<Event>) -> bool {
        if Instant::now() >= self.until {
            return false;
        }
        if !self.told {
            self.told = true;
            tracing::info!("waiting for {host} to wake up");
            let _ = events.send(Event::Waking).await;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
        true
    }
}

/// A connection to the server, and the address this side of it has.
pub(crate) async fn open(
    target: &Target,
    events: &mpsc::Sender<Event>,
    patience: Duration,
) -> Result<(Box<dyn Stream>, SocketAddr), Failure> {
    match &target.via {
        Via::Direct => {
            let stream = tcp(&target.host, target.port, patience)
                .await
                .map_err(Failure::Unreachable)?;
            let local = stream
                .local_addr()
                .map_err(|e| Failure::Unreachable(Unreachable::from_io(&e)))?;
            Ok((Box::new(stream), local))
        }
        Via::Ssh(hop) => ssh::open(hop, target, events).await,
        Via::Gateway(hop) => crate::rdp::gateway::open(hop, target, events).await,
    }
}

/// A TCP connection that notices a network gone away within half a
/// minute, rather than the quarter of an hour TCP gives a silent one.
async fn tcp(host: &str, port: u16, patience: Duration) -> Result<TcpStream, Unreachable> {
    let stream = match tokio::time::timeout(patience, TcpStream::connect((host, port))).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(e)) => return Err(Unreachable::from_io(&e)),
        Err(_) => return Err(Unreachable::NoAnswer),
    };
    let _ = stream.set_nodelay(true);
    let socket = socket2::SockRef::from(&stream);
    let keepalive = socket2::TcpKeepalive::new()
        .with_time(Duration::from_secs(10))
        .with_interval(Duration::from_secs(5))
        .with_retries(3);
    if let Err(e) = socket
        .set_tcp_keepalive(&keepalive)
        .and_then(|()| socket.set_tcp_user_timeout(Some(Duration::from_secs(20))))
    {
        tracing::debug!("setting the connection's timeouts: {e}");
    }
    Ok(stream)
}
