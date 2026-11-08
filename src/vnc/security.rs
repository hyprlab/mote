//! VNC's kinds of security: which to use of those a server offers, and
//! each one's exchange. A session is encrypted unless the connection
//! allows otherwise, and nothing secret goes out before the server's
//! certificate or key, where it has one, is trusted.

use des::Des;
use des::cipher::{BlockCipherEncrypt, KeyInit};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

use super::handshake::{early, read_text};
use super::{apple, dh, openssl, ra2};
use crate::remote::route::Stream;
use crate::remote::{
    Event, Failure, Identity, Protection, Security, Target, Trust, Via, fingerprint,
};

pub(super) const NONE: u8 = 1;
pub(super) const VNC_AUTHENTICATION: u8 = 2;
/// Anonymous TLS around a second choice of security: GNOME Remote
/// Desktop's, and vino's before it.
const TLS: u8 = 18;
const VENCRYPT: u8 = 19;

/// The kinds of security taken, best first. RSA-AES leads, as TigerVNC and
/// wayvnc both offer it beside VeNCrypt and its key says which server it
/// is; choosing the same way each time keeps the identity trusted the
/// same. Apple's SRP is offered by macOS alone, beside its Diffie-Hellman.
const BEST_FIRST: [u8; 10] = [
    ra2::RA2_256,
    ra2::RA2,
    VENCRYPT,
    apple::APPLE_SRP,
    TLS,
    ra2::RA2NE_256,
    ra2::RA2NE,
    dh::APPLE_DH,
    NONE,
    VNC_AUTHENTICATION,
];

/// VeNCrypt's subtypes taken, best first: a certificate, anonymous TLS,
/// then none. Within each, no sign-in first, where the server offers that.
const X509_NONE: u32 = 260;
const X509_VNC: u32 = 261;
const X509_PLAIN: u32 = 262;
const TLS_NONE: u32 = 257;
const TLS_VNC: u32 = 258;
const TLS_PLAIN: u32 = 259;
const PLAIN: u32 = 256;
const SUBTYPES: [u32; 7] = [
    X509_NONE, X509_VNC, X509_PLAIN, TLS_NONE, TLS_VNC, TLS_PLAIN, PLAIN,
];

/// What `kind` makes of a session; `apple` for macOS, which encrypts the
/// session after its Diffie-Hellman too, though without knowing the server.
fn protection(kind: u8, apple: bool) -> Protection {
    match kind {
        ra2::RA2_256 | ra2::RA2 | VENCRYPT | apple::APPLE_SRP => Protection::Verified,
        TLS => Protection::Unverified,
        dh::APPLE_DH if apple => Protection::Unverified,
        ra2::RA2NE_256 | ra2::RA2NE | dh::APPLE_DH => Protection::PasswordOnly,
        _ => Protection::Unencrypted,
    }
}

/// The kind to use of those offered, or why none will do. `apple` for a
/// server that greeted as macOS does.
pub(super) fn choose(offered: &[u8], allow_unencrypted: bool, apple: bool) -> Result<u8, Failure> {
    let usable = |kind: &u8| *kind != TLS || openssl::available();
    let encrypted = |kind: &u8| {
        matches!(
            protection(*kind, apple),
            Protection::Verified | Protection::Unverified
        )
    };
    let known = BEST_FIRST.iter().filter(|kind| offered.contains(kind));
    if let Some(&kind) = known
        .clone()
        .find(|kind| usable(kind) && (allow_unencrypted || encrypted(kind)))
    {
        return Ok(kind);
    }
    let security = if offered.contains(&TLS) && !openssl::available() {
        Security::NoOpenSsl
    } else if known.clone().any(|kind| !encrypted(kind)) {
        Security::Unencrypted
    } else {
        Security::Unsupported(names(offered))
    };
    Err(Failure::Security(security))
}

/// A stream through its kind of security, and what it is like after.
pub(super) struct Secured {
    pub(super) stream: Box<dyn Stream>,
    pub(super) protection: Protection,
    /// Whether the server reports on the sign-in: always, but for None
    /// before RFB 3.8.
    pub(super) reports: bool,
    /// For macOS, the key from the sign-in that its session's first key
    /// comes wrapped in.
    pub(super) wrap: Option<[u8; 16]>,
}

/// Run the exchange of `kind`, chosen and sent already, with a server
/// that is macOS when `apple`.
pub(super) async fn run(
    kind: u8,
    mut stream: Box<dyn Stream>,
    target: &Target,
    events: &mpsc::Sender<Event>,
    asked: &mut bool,
    apple: bool,
) -> Result<Secured, Failure> {
    let password = target.password.as_str();
    let secured = |stream, reports| Secured {
        stream,
        protection: protection(kind, apple),
        reports,
        wrap: None,
    };
    match kind {
        NONE => Ok(secured(stream, false)),
        VNC_AUTHENTICATION => {
            vnc_authentication(&mut stream, password).await?;
            Ok(secured(stream, true))
        }
        TLS => {
            let mut stream: Box<dyn Stream> = Box::new(anonymous(stream).await?);
            // Inside it, a second choice, of None or VNC Authentication.
            let count = stream.read_u8().await.map_err(early)?;
            if count == 0 {
                return Err(Failure::Declined(read_text(&mut stream).await?));
            }
            let mut inner = vec![0; usize::from(count)];
            stream.read_exact(&mut inner).await.map_err(early)?;
            let chosen = [NONE, VNC_AUTHENTICATION]
                .into_iter()
                .find(|kind| inner.contains(kind))
                .ok_or_else(|| Failure::Security(Security::Unsupported(names(&inner))))?;
            send(&mut stream, &[chosen]).await?;
            if chosen == VNC_AUTHENTICATION {
                vnc_authentication(&mut stream, password).await?;
            }
            Ok(secured(stream, true))
        }
        VENCRYPT => vencrypt(stream, target, events, asked).await,
        ra2::RA2 | ra2::RA2NE | ra2::RA2_256 | ra2::RA2NE_256 => {
            let key = ra2::server_key(&mut stream).await?;
            *asked |= trust(target, events, &key.bytes, Identity::ServerKey).await?;
            if password.is_empty() {
                return Err(Failure::Refused);
            }
            let stream = ra2::sign_in(stream, kind, &key, &target.username, password).await?;
            Ok(secured(stream, true))
        }
        dh::APPLE_DH => {
            if target.username.is_empty() || password.is_empty() {
                return Err(Failure::Refused);
            }
            let key = dh::sign_in(&mut stream, &target.username, password).await?;
            Ok(Secured {
                wrap: apple.then_some(key),
                ..secured(stream, true)
            })
        }
        apple::APPLE_SRP => {
            if target.username.is_empty() || password.is_empty() {
                return Err(Failure::Refused);
            }
            let wrap = apple::sign_in(&mut stream, &target.username, password).await?;
            Ok(Secured {
                wrap: Some(wrap),
                ..secured(stream, true)
            })
        }
        other => Err(Failure::Security(Security::Unsupported(names(&[other])))),
    }
}

/// VeNCrypt: a version, then a choice of subtypes, each TLS of some kind
/// or none around a sign-in.
async fn vencrypt(
    mut stream: Box<dyn Stream>,
    target: &Target,
    events: &mpsc::Sender<Event>,
    asked: &mut bool,
) -> Result<Secured, Failure> {
    let major = stream.read_u8().await.map_err(early)?;
    let minor = stream.read_u8().await.map_err(early)?;
    if (major, minor) < (0, 2) {
        return Err(Failure::Security(Security::Unsupported(format!(
            "VeNCrypt {major}.{minor}"
        ))));
    }
    send(&mut stream, &[0, 2]).await?;
    if stream.read_u8().await.map_err(early)? != 0 {
        return Err(Failure::Protocol(
            "the server turned down VeNCrypt 0.2".into(),
        ));
    }
    let count = stream.read_u8().await.map_err(early)?;
    let mut offered = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        offered.push(stream.read_u32().await.map_err(early)?);
    }
    tracing::debug!("VeNCrypt subtypes offered: {offered:?}");
    let anonymous_ok = openssl::available();
    let subtype = SUBTYPES
        .into_iter()
        .filter(|subtype| offered.contains(subtype))
        .find(|&subtype| match subtype {
            TLS_NONE | TLS_VNC | TLS_PLAIN => anonymous_ok,
            PLAIN => target.allow_unencrypted,
            _ => true,
        })
        .ok_or_else(|| {
            Failure::Security(if offered.contains(&PLAIN) {
                Security::Unencrypted
            } else if offered
                .iter()
                .any(|s| matches!(*s, TLS_NONE | TLS_VNC | TLS_PLAIN))
            {
                Security::NoOpenSsl
            } else {
                Security::Unsupported(format!("VeNCrypt {offered:?}"))
            })
        })?;
    send(&mut stream, &subtype.to_be_bytes()).await?;
    let protection = match subtype {
        X509_NONE | X509_VNC | X509_PLAIN => Protection::Verified,
        TLS_NONE | TLS_VNC | TLS_PLAIN => Protection::Unverified,
        _ => Protection::Unencrypted,
    };
    if subtype != PLAIN && stream.read_u8().await.map_err(early)? != 1 {
        return Err(Failure::Protocol(
            "the server turned down its own subtype".into(),
        ));
    }
    let mut stream: Box<dyn Stream> = match protection {
        Protection::Verified => {
            let (tls, certificate) = ironrdp_tls::upgrade_with_certificate_validation(
                stream,
                &target.host,
                ironrdp_tls::CertificateValidation::DangerouslyAcceptInvalidCertificate,
            )
            .await
            .map_err(|e| Failure::Security(Security::Other(e.to_string())))?;
            let der = x509_cert::der::Encode::to_der(&certificate)
                .map_err(|e| Failure::Protocol(e.to_string()))?;
            *asked |= trust(target, events, &der, Identity::Server).await?;
            Box::new(tls)
        }
        Protection::Unverified => Box::new(anonymous(stream).await?),
        _ => stream,
    };
    match subtype {
        X509_VNC | TLS_VNC => vnc_authentication(&mut stream, &target.password).await?,
        X509_PLAIN | TLS_PLAIN | PLAIN => {
            let (username, password) = (target.username.as_bytes(), target.password.as_bytes());
            if username.is_empty() || password.is_empty() {
                return Err(Failure::Refused);
            }
            let mut message = (username.len() as u32).to_be_bytes().to_vec();
            message.extend_from_slice(&(password.len() as u32).to_be_bytes());
            message.extend_from_slice(username);
            message.extend_from_slice(password);
            send(&mut stream, &message).await?;
        }
        _ => {}
    }
    Ok(Secured {
        stream,
        protection,
        reports: true,
        wrap: None,
    })
}

/// Anonymous TLS, or why it couldn't start.
async fn anonymous(stream: Box<dyn Stream>) -> Result<openssl::AnonTls, Failure> {
    openssl::connect(stream).await.map_err(|e| match e {
        openssl::Error::Missing => Failure::Security(Security::NoOpenSsl),
        openssl::Error::Io(e) => early(e),
        openssl::Error::Tls(why) => Failure::Security(Security::Other(why)),
    })
}

/// VNC Authentication: the server's challenge, encrypted with the password.
async fn vnc_authentication(stream: &mut Box<dyn Stream>, password: &str) -> Result<(), Failure> {
    if password.is_empty() {
        return Err(Failure::Refused);
    }
    let mut challenge = [0; 16];
    stream.read_exact(&mut challenge).await.map_err(early)?;
    send(stream, &respond(password, challenge)).await
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

/// Ask the interface whether to trust a certificate or key; `Ok(true)`
/// when the user had to answer, rather than it being known.
async fn trust(
    target: &Target,
    events: &mpsc::Sender<Event>,
    identity: &[u8],
    of: Identity,
) -> Result<bool, Failure> {
    let (reply, answer) = oneshot::channel();
    // Behind a tunnel, the name is the one it goes by there.
    let named = |host: &str| match &target.via {
        Via::Direct => host.to_owned(),
        Via::Ssh(hop) | Via::Gateway(hop) => format!("{host} via {}", hop.host),
    };
    events
        .send(Event::Certificate {
            host: named(&target.host),
            port: target.port,
            fingerprint: fingerprint(identity),
            of,
            also: target
                .addresses
                .iter()
                .map(|(host, port)| (named(host), *port))
                .collect(),
            reply,
        })
        .await
        .map_err(|_| Failure::Untrusted)?;
    match answer.await.unwrap_or(Trust::Rejected) {
        Trust::Known => Ok(false),
        Trust::Accepted => Ok(true),
        Trust::Rejected => Err(Failure::Untrusted),
    }
}

async fn send(stream: &mut Box<dyn Stream>, bytes: &[u8]) -> Result<(), Failure> {
    stream.write_all(bytes).await.map_err(early)?;
    stream.flush().await.map_err(early)
}

/// The security types as people know them, for a server none of whose
/// types this side speaks.
pub(super) fn names(kinds: &[u8]) -> String {
    kinds
        .iter()
        .map(|&kind| match kind {
            1 => "None".to_owned(),
            2 => "VNC Authentication".to_owned(),
            5 | 6 | 129 | 133 => "RSA-AES".to_owned(),
            16 => "Tight".to_owned(),
            18 => "TLS".to_owned(),
            19 => "VeNCrypt".to_owned(),
            20 => "SASL".to_owned(),
            30 | 33 | 36 => "Apple".to_owned(),
            35 => "Kerberos".to_owned(),
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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn chooses_encryption_and_goes_without_only_when_allowed() {
        // wayvnc: VeNCrypt and both RSA-AES.
        assert_eq!(choose(&[19, 129, 5], false, false).ok(), Some(ra2::RA2_256));
        assert_eq!(choose(&[19], false, false).ok(), Some(VENCRYPT));
        // VNC Authentication alone, as Linux Mint's server and KRfb offer.
        assert!(matches!(
            choose(&[2], false, false),
            Err(Failure::Security(Security::Unencrypted))
        ));
        assert_eq!(choose(&[2], true, false).ok(), Some(VNC_AUTHENTICATION));
        // A password protected beats one sent as a challenge's answer.
        assert_eq!(choose(&[2, 30, 6], true, false).ok(), Some(ra2::RA2NE));
        assert!(matches!(
            choose(&[16, 20], true, false),
            Err(Failure::Security(Security::Unsupported(_)))
        ));
    }

    #[test]
    fn takes_apple_srp_from_macos_and_its_diffie_hellman_encrypted() {
        // What macOS 26 offers.
        let macos = [30, 33, 36, 2, 35];
        assert_eq!(choose(&macos, false, true).ok(), Some(apple::APPLE_SRP));
        assert_eq!(choose(&[30, 2], false, true).ok(), Some(dh::APPLE_DH));
        // wayvnc's Diffie-Hellman protects the password alone.
        assert!(choose(&[30], false, false).is_err());
    }
}
