//! An SSH tunnel: the server's port reached through an SSH server, as
//! `ssh -L` reaches it, signing in with the SSH agent's keys, then the
//! keys in `~/.ssh` that have no passphrase, then the password.

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use russh::client;
use russh::keys::agent::client::AgentClient;
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{mpsc, oneshot};

use super::super::{Event, Failure, Hop, Identity, Target, Trust, Tunnel, Unreachable};
use super::Stream;

pub(super) async fn open(
    hop: &Hop,
    target: &Target,
    events: &mpsc::Sender<Event>,
) -> Result<(Box<dyn Stream>, SocketAddr), Failure> {
    let tcp = super::tcp(&hop.host, hop.port, super::TCP_TIMEOUT)
        .await
        .map_err(|why| Failure::Ssh(Tunnel::Unreachable(why)))?;
    let local = tcp
        .local_addr()
        .map_err(|e| Failure::Ssh(Tunnel::Unreachable(Unreachable::from_io(&e))))?;
    let config = Arc::new(client::Config {
        // The SSH server is asked after every so often, so a dropped
        // network ends the tunnel, and the session reconnects.
        keepalive_interval: Some(Duration::from_secs(15)),
        keepalive_max: 3,
        nodelay: true,
        ..client::Config::default()
    });
    let checker = Checker {
        host: hop.host.clone(),
        port: hop.port,
        events: events.clone(),
    };
    let mut session = client::connect_stream(config, tcp, checker)
        .await
        .map_err(|e| match e {
            Error::Untrusted => Failure::Untrusted,
            Error::KeyChanged => Failure::Ssh(Tunnel::KeyChanged),
            Error::Ssh(e) => Failure::Ssh(Tunnel::Other(e.to_string())),
        })?;
    if !sign_in(&mut session, &hop.username, &hop.password).await {
        return Err(Failure::Ssh(Tunnel::Refused));
    }
    tracing::info!("SSH tunnel to {} open", hop.host);
    if leads_to_itself(&hop.host, &target.host)
        && let Some(system) = probe(&session).await
    {
        tracing::debug!("the computer runs {system:?}");
        let _ = events.send(Event::System(system)).await;
    }
    let channel = session
        .channel_open_direct_tcpip(
            target.host.as_str(),
            u32::from(target.port),
            local.ip().to_string(),
            u32::from(local.port()),
        )
        .await
        .map_err(|e| Failure::Ssh(Tunnel::NoRoute(e.to_string())))?;
    let tunnel = Channel {
        stream: channel.into_stream(),
        _session: session,
    };
    Ok((Box::new(tunnel), local))
}

/// Try each way of signing in until one works.
/// Whether the computer is the SSH server itself, so what SSH finds there
/// is the computer's.
fn leads_to_itself(ssh_host: &str, host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1") || host.eq_ignore_ascii_case(ssh_host)
}

/// Which distribution and graphics card the SSH server has, for the advice
/// given when it sends no H.264. Not worth holding the connection up for.
async fn probe(session: &client::Handle<Checker>) -> Option<crate::rdp::server::System> {
    let run = async {
        let mut channel = session.channel_open_session().await.ok()?;
        channel.exec(true, crate::rdp::server::PROBE).await.ok()?;
        let mut output = Vec::new();
        while let Some(message) = channel.wait().await {
            if let russh::ChannelMsg::Data { data } = message {
                output.extend_from_slice(&data);
            }
        }
        Some(crate::rdp::server::System::from_probe(
            &String::from_utf8_lossy(&output),
        ))
    };
    tokio::time::timeout(Duration::from_secs(3), run)
        .await
        .ok()
        .flatten()
}

async fn sign_in(session: &mut client::Handle<Checker>, user: &str, password: &str) -> bool {
    let rsa_hash = session
        .best_supported_rsa_hash()
        .await
        .ok()
        .flatten()
        .flatten();
    let hash = |key: &PublicKey| key.algorithm().is_rsa().then_some(rsa_hash).flatten();
    match AgentClient::connect_env().await {
        Ok(mut agent) => {
            let identities = agent.request_identities().await.unwrap_or_default();
            for identity in identities {
                let key = identity.public_key().into_owned();
                let hash = hash(&key);
                match session
                    .authenticate_publickey_with(user, key, hash, &mut agent)
                    .await
                {
                    Ok(result) if result.success() => return true,
                    Ok(_) => {}
                    Err(e) => tracing::debug!("SSH agent: {e}"),
                }
            }
        }
        Err(e) => tracing::debug!("no SSH agent: {e}"),
    }
    let ssh = gtk::glib::home_dir().join(".ssh");
    for name in ["id_ed25519", "id_ecdsa", "id_rsa"] {
        // One with a passphrase is for the agent to unlock.
        let Ok(key) = russh::keys::load_secret_key(ssh.join(name), None) else {
            continue;
        };
        let hash = hash(key.public_key());
        let key = PrivateKeyWithHashAlg::new(Arc::new(key), hash);
        if let Ok(result) = session.authenticate_publickey(user, key).await
            && result.success()
        {
            return true;
        }
    }
    if !password.is_empty()
        && let Ok(result) = session.authenticate_password(user, password).await
    {
        return result.success();
    }
    false
}

/// Checks the SSH server's key against `~/.ssh/known_hosts`, and asks
/// about one that isn't there, as `ssh` would on a terminal.
struct Checker {
    host: String,
    port: u16,
    events: mpsc::Sender<Event>,
}

#[derive(Debug)]
enum Error {
    Ssh(russh::Error),
    Untrusted,
    KeyChanged,
}

impl From<russh::Error> for Error {
    fn from(error: russh::Error) -> Self {
        Self::Ssh(error)
    }
}

impl client::Handler for Checker {
    type Error = Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Error> {
        let key: PublicKey = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            PublicKeyOrCertificate::Certificate(certificate) => {
                PublicKey::new(certificate.public_key().clone(), "")
            }
        };
        match russh::keys::check_known_hosts(&self.host, self.port, &key) {
            Ok(true) => return Ok(true),
            Err(russh::keys::Error::KeyChanged { line }) => {
                tracing::warn!(
                    "the SSH server's key differs from line {line} of ~/.ssh/known_hosts"
                );
                return Err(Error::KeyChanged);
            }
            // Not there, or no file to look in.
            _ => {}
        }
        let (reply, answer) = oneshot::channel();
        let asked = Event::Certificate {
            host: self.host.clone(),
            port: self.port,
            fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
            of: Identity::Ssh,
            also: Vec::new(),
            reply,
        };
        if self.events.send(asked).await.is_err() {
            return Err(Error::Untrusted);
        }
        match answer.await.unwrap_or(Trust::Rejected) {
            Trust::Known | Trust::Accepted => Ok(true),
            Trust::Rejected => Err(Error::Untrusted),
        }
    }
}

/// The forwarded connection, which keeps the SSH session open for as long
/// as it is.
struct Channel {
    stream: russh::ChannelStream<client::Msg>,
    _session: client::Handle<Checker>,
}

impl AsyncRead for Channel {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for Channel {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
