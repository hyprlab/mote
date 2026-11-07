//! The RFB handshake: the protocol version, the security type and its
//! exchange (in `security`), and the server's description of its desktop
//! (RFC 6143, 7.1 to 7.3). The stream passes through by value, as a kind
//! of security can hand back another around it.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use super::security::{self, NONE};
use crate::remote::route::Stream;
use crate::remote::{Event, Failure, Protection, Target, Unreachable, describe};

/// How long a server gets to say anything at all: an RDP server, or
/// anything else that waits for the client to speak first, never does.
/// After the greeting nothing is timed: a server may wait on someone at
/// its desk to let the connection in, and closing the session stops it.
const GREETING_WITHIN: Duration = Duration::from_secs(10);
/// The longest desktop name or reason taken from a server.
const MAX_TEXT: u32 = 4096;
/// The largest desktop taken, on either side: anything larger is more
/// likely damage than a desktop.
pub(super) const MAX_SIDE: u16 = 16384;

/// What the server says of its desktop once the client is in.
#[derive(Debug)]
pub(super) struct ServerInit {
    pub(super) width: u16,
    pub(super) height: u16,
}

/// A connection signed in to, ready for the session.
pub(super) struct Opened {
    pub(super) stream: Box<dyn Stream>,
    pub(super) init: ServerInit,
    pub(super) protection: Protection,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Version {
    V3_3,
    V3_7,
    V3_8,
}

/// Sign in to `target`, sharing the desktop with anyone else on it.
/// `asked` is set when the user was asked to trust the server, which a
/// server may not wait for.
pub(super) async fn run(
    mut stream: Box<dyn Stream>,
    target: &Target,
    events: &mpsc::Sender<Event>,
    asked: &mut bool,
) -> Result<Opened, Failure> {
    let mut greeting = [0; 12];
    match tokio::time::timeout(GREETING_WITHIN, stream.read_exact(&mut greeting)).await {
        Err(_) => return Err(Failure::Unreachable(Unreachable::NotServer)),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Err(Failure::Unreachable(Unreachable::Closed));
        }
        Ok(Err(e)) => return Err(Failure::Unreachable(Unreachable::from_io(&e))),
        Ok(Ok(_)) => {}
    }
    let version = version(&greeting).ok_or(Failure::Unreachable(Unreachable::NotServer))?;
    tracing::info!(
        "VNC server: {}, speaking {version:?}",
        String::from_utf8_lossy(&greeting[..11])
    );
    let sent: &[u8; 12] = match version {
        Version::V3_3 => b"RFB 003.003\n",
        Version::V3_7 => b"RFB 003.007\n",
        Version::V3_8 => b"RFB 003.008\n",
    };
    stream.write_all(sent).await.map_err(early)?;

    let offered = if version == Version::V3_3 {
        // The server chooses, and says only which.
        match read_u32(&mut stream).await? {
            0 => return Err(Failure::Declined(read_text(&mut stream).await?)),
            kind => vec![
                u8::try_from(kind)
                    .map_err(|_| Failure::Protocol(format!("security type {kind}")))?,
            ],
        }
    } else {
        let count = stream.read_u8().await.map_err(early)?;
        if count == 0 {
            return Err(Failure::Declined(read_text(&mut stream).await?));
        }
        let mut offered = vec![0; usize::from(count)];
        stream.read_exact(&mut offered).await.map_err(early)?;
        offered
    };
    tracing::info!("security types offered: {}", security::names(&offered));
    // In tests, `MOTE_TEST_SECURITY=<type>` takes that kind alone, if it is
    // offered.
    #[cfg(test)]
    let offered: Vec<u8> = match std::env::var("MOTE_TEST_SECURITY")
        .ok()
        .and_then(|k| k.parse::<u8>().ok())
    {
        Some(only) => offered.into_iter().filter(|&kind| kind == only).collect(),
        None => offered,
    };
    let kind = security::choose(&offered, target.allow_unencrypted)?;
    if version != Version::V3_3 {
        stream.write_all(&[kind]).await.map_err(early)?;
        stream.flush().await.map_err(early)?;
    }
    let secured = security::run(kind, stream, target, events, asked).await?;
    let mut stream = secured.stream;
    // RFB 3.8 reports on None too; the versions before it don't.
    if (secured.reports || (kind == NONE && version == Version::V3_8))
        && read_u32(&mut stream).await? != 0
    {
        let reason = if version == Version::V3_8 {
            read_text(&mut stream).await?
        } else {
            String::new()
        };
        return Err(refused(reason));
    }
    tracing::info!(
        "signed in with {}, {:?}",
        security::names(&[kind]),
        secured.protection
    );

    // Shared: anyone else on the desktop stays on it.
    stream.write_all(&[1]).await.map_err(early)?;
    stream.flush().await.map_err(early)?;
    let width = read_u16(&mut stream).await?;
    let height = read_u16(&mut stream).await?;
    // The server's own pixel format; the session asks for its own.
    let mut format = [0; 16];
    stream.read_exact(&mut format).await.map_err(early)?;
    let name = read_text(&mut stream).await?;
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(Failure::Protocol(format!("a desktop of {width}x{height}")));
    }
    tracing::info!("in “{name}”, {width}x{height}");
    Ok(Opened {
        stream,
        init: ServerInit { width, height },
        protection: secured.protection,
    })
}

/// The version to speak with a server that greets with `greeting`. Apple
/// sends 3.889 and RealVNC 4.x and 5.x, all of which speak 3.8; anything
/// else unknown is 3.3, as RFC 6143 says.
fn version(greeting: &[u8; 12]) -> Option<Version> {
    let text = std::str::from_utf8(greeting).ok()?;
    let digits = text.strip_prefix("RFB ")?.strip_suffix('\n')?;
    let (major, minor) = digits.split_once('.')?;
    let (major, minor): (u32, u32) = (major.parse().ok()?, minor.parse().ok()?);
    Some(match (major, minor) {
        (3, 7) => Version::V3_7,
        (3, 8..) | (4.., _) => Version::V3_8,
        _ => Version::V3_3,
    })
}

/// A sign-in the server turned down. TigerVNC locks a client out after
/// several, and says so.
fn refused(reason: String) -> Failure {
    tracing::info!("the server refused the sign-in: “{reason}”");
    if reason.to_lowercase().contains("too many") {
        Failure::Declined(reason)
    } else {
        Failure::Refused
    }
}

/// An I/O error before the session starts: a server that closes the
/// connection at this point turns the client away without a reason.
pub(super) fn early(error: std::io::Error) -> Failure {
    if error.kind() == std::io::ErrorKind::UnexpectedEof {
        Failure::Rejected
    } else {
        Failure::Lost(describe(&error))
    }
}

async fn read_u16(stream: &mut Box<dyn Stream>) -> Result<u16, Failure> {
    stream.read_u16().await.map_err(early)
}

async fn read_u32(stream: &mut Box<dyn Stream>) -> Result<u32, Failure> {
    stream.read_u32().await.map_err(early)
}

/// A length and that many bytes of text, as reasons and names come.
pub(super) async fn read_text(stream: &mut Box<dyn Stream>) -> Result<String, Failure> {
    let length = read_u32(stream).await?;
    if length > MAX_TEXT {
        return Err(Failure::Protocol(format!("{length} bytes of text")));
    }
    let mut text = vec![0; length as usize];
    stream.read_exact(&mut text).await.map_err(early)?;
    Ok(String::from_utf8_lossy(&text).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_versions_servers_greet_with() {
        let read = |text: &[u8; 12]| version(text);
        assert_eq!(read(b"RFB 003.008\n"), Some(Version::V3_8));
        assert_eq!(read(b"RFB 003.007\n"), Some(Version::V3_7));
        assert_eq!(read(b"RFB 003.003\n"), Some(Version::V3_3));
        assert_eq!(read(b"RFB 003.889\n"), Some(Version::V3_8));
        assert_eq!(read(b"RFB 004.001\n"), Some(Version::V3_8));
        assert_eq!(read(b"RFB 003.005\n"), Some(Version::V3_3));
        assert_eq!(read(b"SSH-2.0-Open"), None);
    }
}
