//! macOS Screen Sharing, which greets as RFB 3.889 and offers Apple's own
//! kinds of security beside VNC Authentication. Each macOS version takes
//! some of them: the newest, SRP, verifies the Mac; Apple's RSA and
//! Diffie-Hellman sign-ins don't, and Diffie-Hellman alone is all the
//! oldest take. After any of them, macOS can encrypt the whole session in
//! records (see `records`). The ways are tried best first, each on a
//! connection of its own, until macOS takes one. Apple documents none of
//! it; the layouts are those of iShareScreen's description of the
//! protocol, checked against macOS 27.

pub(super) mod pasteboard;
mod records;
pub(super) mod srp;

pub(super) use records::{ENCRYPTION_INFO, MISC_STATUS, Rekeys, start};

use aes::Aes128;
use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockEncrypt, KeyInit};
use rsa::pkcs1::DecodeRsaPublicKey;
use rsa::pkcs8::DecodePublicKey;
use rsa::rand_core::{OsRng, RngCore};
use rsa::{Pkcs1v15Encrypt, RsaPublicKey};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::dh::{self, APPLE_DH};
use super::handshake::early;
use crate::remote::route::Stream;
use crate::remote::{Failure, Protection};

/// Apple's RSA sign-ins, with SRP or without.
pub(super) const APPLE_RSA: u8 = 33;

/// The longest message taken from the server during a sign-in.
const MAX_MESSAGE: u32 = 8192;

/// The ways into a Mac, best first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(super) enum Way {
    /// SRP, which verifies the Mac, and records.
    Srp,
    /// The account encrypted under a key sent under the server's RSA key,
    /// and records. macOS shows no fingerprint of that key to check it by.
    Rsa,
    /// Diffie-Hellman, and records.
    DhRecords,
    /// Diffie-Hellman alone, which protects the password and nothing else.
    Dh,
}

impl Way {
    const ALL: [Self; 4] = [Self::Srp, Self::Rsa, Self::DhRecords, Self::Dh];

    pub(super) fn kind(self) -> u8 {
        match self {
            Self::Srp | Self::Rsa => APPLE_RSA,
            Self::DhRecords | Self::Dh => APPLE_DH,
        }
    }

    pub(super) fn records(self) -> bool {
        self != Self::Dh
    }

    pub(super) fn protection(self) -> Protection {
        match self {
            Self::Srp => Protection::Verified,
            Self::Rsa | Self::DhRecords => Protection::Unverified,
            Self::Dh => Protection::PasswordOnly,
        }
    }
}

/// What a session has learned of a Mac across its connections: the ways
/// it turned down are not tried again, reconnecting included.
#[derive(Default)]
pub(super) struct Ways {
    /// The best way still worth trying.
    from: Option<Way>,
    /// The way the connection under way is trying.
    trying: Option<Way>,
    /// The Mac didn't put a session in records.
    no_records: bool,
}

impl Ways {
    /// The way to try with a Mac offering `offered`, or none where none of
    /// them is offered and allowed: then VNC Authentication, if anything.
    pub(super) fn choose(&mut self, offered: &[u8], allow_unencrypted: bool) -> Option<Way> {
        // In tests, `MOTE_TEST_APPLE=<srp|rsa|dhrecords|dh>` starts there.
        #[cfg(test)]
        if self.from.is_none() {
            self.from = std::env::var("MOTE_TEST_APPLE").ok().and_then(|way| {
                Way::ALL
                    .into_iter()
                    .find(|w| format!("{w:?}").eq_ignore_ascii_case(&way))
            });
        }
        let from = self.from.unwrap_or(Way::Srp);
        self.trying = Way::ALL.into_iter().find(|way| {
            *way >= from
                && offered.contains(&way.kind())
                && !(self.no_records && way.records())
                && (allow_unencrypted || way.protection() != Protection::PasswordOnly)
        });
        self.trying
    }

    /// The Mac took the sign-in and not the records.
    pub(super) fn no_records(&mut self) {
        self.no_records = true;
    }

    /// The connection failed with `failure`: whether the next way is worth
    /// a connection of its own. A Mac turns down a way it doesn't have by
    /// closing the connection or sending what this side can't read; a
    /// wrong password, refused, ends the tries, as does a Mac out of reach.
    pub(super) fn failed(&mut self, failure: &Failure) -> bool {
        let Some(way) = self.trying.take() else {
            return false;
        };
        if !matches!(
            failure,
            Failure::Rejected | Failure::Protocol(_) | Failure::Lost(_)
        ) {
            return false;
        }
        let Some(next) = Way::ALL
            .into_iter()
            .find(|next| *next > way && !(self.no_records && next.records()))
        else {
            return false;
        };
        tracing::info!("macOS turned down {way:?}; trying {next:?}");
        self.from = Some(next);
        true
    }
}

/// The server's RSA key, as it came.
pub(super) struct ServerKey {
    key: RsaPublicKey,
}

/// Choose Apple's RSA type and ask for the server's key. macOS takes the
/// choice only in the same packet as this first message.
pub(super) async fn server_key(stream: &mut Box<dyn Stream>) -> Result<ServerKey, Failure> {
    let mut ask = vec![APPLE_RSA, 0, 0, 0, 10];
    ask.extend_from_slice(&envelope(0));
    ask.extend_from_slice(&[0, 0]);
    send(stream, &ask).await?;
    let answer = message(stream).await?;
    // Two bytes, a length, and the key in DER.
    let bad = || Failure::Protocol("the server's RSA key".into());
    let length = answer.get(2..6).ok_or_else(bad)?;
    let length = u32::from_be_bytes(length.try_into().unwrap()) as usize;
    let der = answer.get(6..6 + length).ok_or_else(bad)?;
    let key = RsaPublicKey::from_public_key_der(der)
        .or_else(|_| RsaPublicKey::from_pkcs1_der(der))
        .map_err(|e| Failure::Protocol(format!("the server's RSA key: {e}")))?;
    Ok(ServerKey { key })
}

/// Apple's RSA sign-in without SRP: the user name and password encrypted
/// with a new AES key, sent under the server's RSA key, which is what the
/// session's first key comes wrapped in. macOS closes the connection on a
/// wrong password, and sends its verdict on a right one.
pub(super) async fn rsa_sign_in(
    stream: &mut Box<dyn Stream>,
    key: &ServerKey,
    username: &str,
    password: &str,
) -> Result<[u8; 16], Failure> {
    let mut secret = [0; 16];
    OsRng.fill_bytes(&mut secret);
    let mut account = dh::credentials(username, password)?;
    let cipher = Aes128::new(&secret.into());
    for block in account.as_chunks_mut::<16>().0 {
        cipher.encrypt_block(GenericArray::from_mut_slice(block));
    }
    let sealed = key
        .key
        .encrypt(&mut OsRng, Pkcs1v15Encrypt, &secret)
        .map_err(|e| Failure::Protocol(format!("the server's RSA key: {e}")))?;
    let mut body = envelope(1).to_vec();
    body.extend_from_slice(&account);
    body.extend_from_slice(&1u16.to_be_bytes());
    body.extend_from_slice(&sealed);
    send(stream, &framed(body)).await?;
    // An empty message before the verdict.
    message(stream).await?;
    Ok(secret)
}

/// The start of every message of the sign-in: a version, and what it is.
fn envelope(kind: u16) -> [u8; 8] {
    let [high, low] = kind.to_be_bytes();
    [1, 0, b'R', b'S', b'A', b'1', high, low]
}

/// A message with its length before it.
fn framed(body: Vec<u8>) -> Vec<u8> {
    let mut out = (body.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(&body);
    out
}

async fn send(stream: &mut Box<dyn Stream>, bytes: &[u8]) -> Result<(), Failure> {
    stream.write_all(bytes).await.map_err(early)?;
    stream.flush().await.map_err(early)
}

/// One of the server's messages: a length, and that many bytes.
async fn message(stream: &mut Box<dyn Stream>) -> Result<Vec<u8>, Failure> {
    let length = stream.read_u32().await.map_err(early)?;
    if length > MAX_MESSAGE {
        return Err(Failure::Protocol(format!(
            "a sign-in message of {length} bytes"
        )));
    }
    let mut body = vec![0; length as usize];
    stream.read_exact(&mut body).await.map_err(early)?;
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tries_the_ways_a_mac_offers_best_first_and_steps_down() {
        // What macOS 27 offers.
        let offered = [30, 33, 36, 2, 35];
        let mut ways = Ways::default();
        assert_eq!(ways.choose(&offered, false), Some(Way::Srp));
        // A wrong password ends the tries.
        assert!(!ways.failed(&Failure::Refused));
        assert_eq!(ways.choose(&offered, false), Some(Way::Srp));
        // A Mac without SRP closes the connection.
        assert!(ways.failed(&Failure::Rejected));
        assert_eq!(ways.choose(&offered, false), Some(Way::Rsa));
        // One that signs in and keeps the session out of records.
        ways.no_records();
        assert!(ways.failed(&Failure::Protocol("no key".into())));
        // Diffie-Hellman alone protects the password only.
        assert_eq!(ways.choose(&offered, false), None);
        assert_eq!(ways.choose(&offered, true), Some(Way::Dh));
        assert!(!ways.failed(&Failure::Rejected));
    }

    #[test]
    fn an_older_mac_without_rsa_starts_at_diffie_hellman() {
        let mut ways = Ways::default();
        assert_eq!(ways.choose(&[30, 35], false), Some(Way::DhRecords));
        assert_eq!(ways.choose(&[2], true), None);
    }
}
