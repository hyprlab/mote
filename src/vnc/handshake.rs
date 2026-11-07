//! The RFB handshake: the protocol version, the security type and its
//! exchange, and the server's description of its desktop (RFC 6143,
//! 7.1 to 7.3). Section 14 adds the encrypted security types; until then
//! a server that asks only for those is turned away with their names.

use std::time::Duration;

use des::Des;
use des::cipher::{BlockCipherEncrypt, KeyInit};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::remote::route::Stream;
use crate::remote::{Failure, Security, Unreachable, describe};

/// How long a server gets to say anything at all: an RDP server, or
/// anything else that waits for the client to speak first, never does.
const GREETING_WITHIN: Duration = Duration::from_secs(10);
/// How long the rest of the handshake may take.
const HANDSHAKE_WITHIN: Duration = Duration::from_secs(30);
/// The longest desktop name or reason taken from a server.
const MAX_TEXT: u32 = 4096;
/// The largest desktop taken, on either side: anything larger is more
/// likely damage than a desktop.
pub(super) const MAX_SIDE: u16 = 16384;

const NONE: u8 = 1;
const VNC_AUTHENTICATION: u8 = 2;

/// What the server says of its desktop once the client is in.
#[derive(Debug)]
pub(super) struct ServerInit {
    pub(super) width: u16,
    pub(super) height: u16,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Version {
    V3_3,
    V3_7,
    V3_8,
}

/// Sign in with `password`, sharing the desktop with anyone else on it.
pub(super) async fn run(
    stream: &mut Box<dyn Stream>,
    password: &str,
) -> Result<ServerInit, Failure> {
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
    match tokio::time::timeout(HANDSHAKE_WITHIN, rest(stream, version, password)).await {
        Ok(result) => result,
        Err(_) => Err(Failure::Protocol("the server stopped answering".into())),
    }
}

async fn rest(
    stream: &mut Box<dyn Stream>,
    version: Version,
    password: &str,
) -> Result<ServerInit, Failure> {
    let sent: &[u8; 12] = match version {
        Version::V3_3 => b"RFB 003.003\n",
        Version::V3_7 => b"RFB 003.007\n",
        Version::V3_8 => b"RFB 003.008\n",
    };
    stream.write_all(sent).await.map_err(early)?;

    let chosen = if version == Version::V3_3 {
        // The server chooses, and says only which.
        match read_u32(stream).await? {
            0 => return Err(Failure::Declined(read_text(stream).await?)),
            kind => u8::try_from(kind)
                .map_err(|_| Failure::Protocol(format!("security type {kind}")))?,
        }
    } else {
        let count = read_u8(stream).await?;
        if count == 0 {
            return Err(Failure::Declined(read_text(stream).await?));
        }
        let mut offered = vec![0; usize::from(count)];
        stream.read_exact(&mut offered).await.map_err(early)?;
        tracing::debug!("security types offered: {offered:?}");
        // None gives the server nothing; VNC Authentication sends a
        // response to a challenge, never the password itself.
        let chosen = [NONE, VNC_AUTHENTICATION]
            .into_iter()
            .find(|kind| offered.contains(kind))
            .ok_or_else(|| Failure::Security(Security::Unsupported(names(&offered))))?;
        stream.write_all(&[chosen]).await.map_err(early)?;
        chosen
    };
    match chosen {
        NONE => {}
        VNC_AUTHENTICATION => {
            if password.is_empty() {
                return Err(Failure::Refused);
            }
            let mut challenge = [0; 16];
            stream.read_exact(&mut challenge).await.map_err(early)?;
            stream
                .write_all(&respond(password, challenge))
                .await
                .map_err(early)?;
        }
        other => {
            return Err(Failure::Security(Security::Unsupported(names(&[other]))));
        }
    }
    // RFB 3.8 reports on None too; the versions before it only on the
    // others.
    if chosen != NONE || version == Version::V3_8 {
        match read_u32(stream).await? {
            0 => {}
            _ => {
                let reason = if version == Version::V3_8 {
                    read_text(stream).await?
                } else {
                    String::new()
                };
                return Err(refused(reason));
            }
        }
    }

    // Shared: anyone else on the desktop stays on it.
    stream.write_all(&[1]).await.map_err(early)?;
    let width = read_u16(stream).await?;
    let height = read_u16(stream).await?;
    // The server's own pixel format; the session asks for its own.
    let mut format = [0; 16];
    stream.read_exact(&mut format).await.map_err(early)?;
    let name = read_text(stream).await?;
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(Failure::Protocol(format!("a desktop of {width}x{height}")));
    }
    tracing::info!("signed in to “{name}”, {width}x{height}");
    Ok(ServerInit { width, height })
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

/// VNC Authentication's answer: the challenge encrypted with DES, keyed by
/// the password's first 8 bytes, each byte's bits in reverse order.
fn respond(password: &str, mut challenge: [u8; 16]) -> [u8; 16] {
    let mut key = [0; 8];
    for (k, byte) in key.iter_mut().zip(password.bytes()) {
        *k = byte.reverse_bits();
    }
    let cipher = Des::new(&key.into());
    for block in challenge.as_chunks_mut::<8>().0 {
        cipher.encrypt_block(block.into());
    }
    challenge
}

/// The security types as people know them, for a server none of whose
/// types this side speaks.
fn names(kinds: &[u8]) -> String {
    kinds
        .iter()
        .map(|&kind| match kind {
            5 | 6 | 129 | 133 => "RSA-AES".to_owned(),
            16 => "Tight".to_owned(),
            18 => "TLS".to_owned(),
            19 => "VeNCrypt".to_owned(),
            20 => "SASL".to_owned(),
            30 => "Apple".to_owned(),
            other => other.to_string(),
        })
        .fold(Vec::<String>::new(), |mut names, name| {
            if !names.contains(&name) {
                names.push(name);
            }
            names
        })
        .join(", ")
}

/// An I/O error before the session starts: a server that closes the
/// connection at this point turns the client away without a reason.
fn early(error: std::io::Error) -> Failure {
    if error.kind() == std::io::ErrorKind::UnexpectedEof {
        Failure::Rejected
    } else {
        Failure::Lost(describe(&error))
    }
}

async fn read_u8(stream: &mut Box<dyn Stream>) -> Result<u8, Failure> {
    stream.read_u8().await.map_err(early)
}

async fn read_u16(stream: &mut Box<dyn Stream>) -> Result<u16, Failure> {
    stream.read_u16().await.map_err(early)
}

async fn read_u32(stream: &mut Box<dyn Stream>) -> Result<u32, Failure> {
    stream.read_u32().await.map_err(early)
}

/// A length and that many bytes of text, as reasons and names come.
async fn read_text(stream: &mut Box<dyn Stream>) -> Result<String, Failure> {
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

    #[test]
    fn answers_a_challenge_as_vnc_servers_check_it() {
        // The answer computed apart, with Python's `cryptography` and the
        // key's bits reversed by hand.
        let challenge = [
            0x8e, 0x3b, 0x29, 0x55, 0x1c, 0xf0, 0x6b, 0x0a, 0x7d, 0x42, 0x91, 0x13, 0xa5, 0x60,
            0xdc, 0x38,
        ];
        assert_eq!(
            respond("mote", challenge),
            [
                0xa3, 0x5d, 0x52, 0x0b, 0x9b, 0x98, 0xa4, 0x45, 0xf3, 0x54, 0xc3, 0x61, 0x95, 0xf8,
                0x33, 0x63
            ]
        );
        // Only the first 8 bytes of a password count.
        assert_eq!(
            respond("12345678", challenge),
            respond("123456789", challenge)
        );
    }

    #[test]
    fn names_each_security_type_once() {
        assert_eq!(names(&[19, 5, 6, 30]), "VeNCrypt, RSA-AES, Apple");
    }
}
