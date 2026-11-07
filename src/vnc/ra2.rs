//! RSA-AES (RA2), the security types TigerVNC and wayvnc share: an RSA
//! exchange that identifies the server by its key, then AES in EAX mode
//! for everything after. RA2 and RA2_256 encrypt the whole session; RA2ne
//! and RA2ne_256 only the sign-in. The key is asked about the way an RDP
//! server's certificate is, before any credentials go out.

use std::io;
use std::pin::Pin;
use std::sync::OnceLock;
use std::task::{Context, Poll, ready};

use aes::{Aes128, Aes256};
use eax::Eax;
use eax::aead::generic_array::GenericArray;
use eax::aead::{AeadInPlace, KeyInit};
use rsa::rand_core::{OsRng, RngCore};
use rsa::traits::PublicKeyParts;
use rsa::{BigUint, Pkcs1v15Encrypt, RsaPrivateKey, RsaPublicKey};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

use crate::remote::Failure;
use crate::remote::route::Stream;

pub(super) const RA2: u8 = 5;
pub(super) const RA2NE: u8 = 6;
pub(super) const RA2_256: u8 = 129;
pub(super) const RA2NE_256: u8 = 133;

/// The client's own key size, as TigerVNC's viewer uses.
const CLIENT_BITS: usize = 2048;
/// The largest message sent at once.
const CHUNK: usize = 8192;

/// The server's public key, as it came.
pub(super) struct ServerKey {
    key: RsaPublicKey,
    /// Its length, modulus and exponent, as the hashes take them.
    pub(super) bytes: Vec<u8>,
}

/// Read the server's key, the first thing it sends.
pub(super) async fn server_key(stream: &mut Box<dyn Stream>) -> Result<ServerKey, Failure> {
    let bits = stream.read_u32().await.map_err(lost)?;
    if !(1024..=8192).contains(&bits) {
        return Err(Failure::Protocol(format!("an RSA key of {bits} bits")));
    }
    let length = (bits as usize).div_ceil(8);
    let mut parts = vec![0; length * 2];
    stream.read_exact(&mut parts).await.map_err(lost)?;
    let (modulus, exponent) = parts.split_at(length);
    let key = RsaPublicKey::new_with_max_size(
        BigUint::from_bytes_be(modulus),
        BigUint::from_bytes_be(exponent),
        8192,
    )
    .map_err(|e| Failure::Protocol(format!("the server's RSA key: {e}")))?;
    let mut bytes = bits.to_be_bytes().to_vec();
    bytes.extend_from_slice(&parts);
    Ok(ServerKey { key, bytes })
}

/// This side's key: one for as long as the app runs, as making one takes
/// a moment. It only ever decrypts the server's random number, which the
/// server chooses afresh each time.
fn client_key() -> Result<&'static (RsaPrivateKey, Vec<u8>), Failure> {
    static KEY: OnceLock<Result<(RsaPrivateKey, Vec<u8>), String>> = OnceLock::new();
    KEY.get_or_init(|| {
        let key = RsaPrivateKey::new(&mut OsRng, CLIENT_BITS).map_err(|e| e.to_string())?;
        let length = CLIENT_BITS / 8;
        let mut bytes = (CLIENT_BITS as u32).to_be_bytes().to_vec();
        bytes.extend(padded(&key.n().to_bytes_be(), length));
        bytes.extend(padded(&key.e().to_bytes_be(), length));
        Ok((key, bytes))
    })
    .as_ref()
    .map_err(|e| Failure::Protocol(format!("making an RSA key: {e}")))
}

/// Finish the exchange once the server's key is trusted, and sign in.
/// What comes back is the connection to go on with: encrypted for RA2 and
/// RA2_256, as it was for the others.
pub(super) async fn sign_in(
    mut stream: Box<dyn Stream>,
    kind: u8,
    server: &ServerKey,
    username: &str,
    password: &str,
) -> Result<Box<dyn Stream>, Failure> {
    let (client, client_bytes) = client_key()?;
    stream.write_all(client_bytes).await.map_err(lost)?;
    stream.flush().await.map_err(lost)?;

    let length = stream.read_u16().await.map_err(lost)?;
    if usize::from(length) != CLIENT_BITS / 8 {
        return Err(Failure::Protocol(format!("{length} bytes of RSA")));
    }
    let mut sealed = vec![0; length.into()];
    stream.read_exact(&mut sealed).await.map_err(lost)?;
    let wide = matches!(kind, RA2_256 | RA2NE_256);
    // As long as the AES key: RFB's description gives 16 bytes alone, and
    // TigerVNC and neatvnc both send 32 for the 256-bit kinds.
    let random_length = if wide { 32 } else { 16 };
    let server_random = client
        .decrypt(Pkcs1v15Encrypt, &sealed)
        .map_err(|e| Failure::Protocol(format!("the server's random number: {e}")))?;
    if server_random.len() != random_length {
        return Err(Failure::Protocol(format!(
            "a random number of {} bytes",
            server_random.len()
        )));
    }
    let mut client_random = vec![0; random_length];
    OsRng.fill_bytes(&mut client_random);
    let sealed = server
        .key
        .encrypt(&mut OsRng, Pkcs1v15Encrypt, &client_random)
        .map_err(|e| Failure::Protocol(format!("sealing a random number: {e}")))?;
    let mut message = (sealed.len() as u16).to_be_bytes().to_vec();
    message.extend(sealed);
    stream.write_all(&message).await.map_err(lost)?;
    stream.flush().await.map_err(lost)?;

    let hash = |a: &[u8], b: &[u8]| -> Vec<u8> {
        if wide {
            Sha256::new()
                .chain_update(a)
                .chain_update(b)
                .finalize()
                .to_vec()
        } else {
            Sha1::new()
                .chain_update(a)
                .chain_update(b)
                .finalize()
                .to_vec()
        }
    };
    let key = |hash: Vec<u8>| -> Cipher {
        if wide {
            Cipher::Wide(Eax::new(GenericArray::from_slice(&hash)))
        } else {
            Cipher::Narrow(Eax::new(GenericArray::from_slice(&hash[..16])))
        }
    };
    let mut channel = Channel {
        to_server: key(hash(&server_random, &client_random)),
        from_server: key(hash(&client_random, &server_random)),
        sent: 0,
        received: 0,
    };

    let theirs = channel.receive(&mut stream).await?;
    if theirs != hash(&server.bytes, client_bytes) {
        return Err(Failure::Protocol(
            "the server's RSA-AES hash didn't match".into(),
        ));
    }
    channel
        .send(&mut stream, &hash(client_bytes, &server.bytes))
        .await?;
    let wants = channel.receive(&mut stream).await?;
    let username = match wants.as_slice() {
        [1] if username.is_empty() => return Err(Failure::Refused),
        [1] => username,
        [2] => "",
        other => return Err(Failure::Protocol(format!("RSA-AES subtype {other:?}"))),
    };
    let (username, password) = (username.as_bytes(), password.as_bytes());
    if username.len() > 255 || password.len() > 255 {
        return Err(Failure::Protocol(
            "a user name or password over 255 bytes".into(),
        ));
    }
    let mut credentials = vec![username.len() as u8];
    credentials.extend_from_slice(username);
    credentials.push(password.len() as u8);
    credentials.extend_from_slice(password);
    channel.send(&mut stream, &credentials).await?;

    Ok(match kind {
        RA2 | RA2_256 => Box::new(Ra2Stream {
            inner: stream,
            channel,
            raw: Vec::new(),
            plain: Vec::new(),
            outgoing: Vec::new(),
        }),
        _ => stream,
    })
}

enum Cipher {
    Narrow(Eax<Aes128>),
    Wide(Eax<Aes256>),
}

/// Both directions' keys, and how many messages each has carried: the
/// count, little-endian, is each message's nonce.
struct Channel {
    to_server: Cipher,
    from_server: Cipher,
    sent: u128,
    received: u128,
}

impl Channel {
    /// A message: its length, which is also authenticated, the ciphertext
    /// and the tag.
    fn seal(&mut self, plain: &[u8]) -> Vec<u8> {
        let header = (plain.len() as u16).to_be_bytes();
        let nonce = self.sent.to_le_bytes();
        self.sent += 1;
        let mut body = plain.to_vec();
        let nonce = GenericArray::from_slice(&nonce);
        let tag = match &self.to_server {
            Cipher::Narrow(c) => c.encrypt_in_place_detached(nonce, &header, &mut body),
            Cipher::Wide(c) => c.encrypt_in_place_detached(nonce, &header, &mut body),
        }
        .expect("EAX encrypts any length");
        let mut message = header.to_vec();
        message.extend(body);
        message.extend_from_slice(&tag);
        message
    }

    fn open(&mut self, header: [u8; 2], mut body: Vec<u8>, tag: &[u8]) -> io::Result<Vec<u8>> {
        let nonce = self.received.to_le_bytes();
        self.received += 1;
        let (nonce, tag) = (
            GenericArray::from_slice(&nonce),
            GenericArray::from_slice(tag),
        );
        match &self.from_server {
            Cipher::Narrow(c) => c.decrypt_in_place_detached(nonce, &header, &mut body, tag),
            Cipher::Wide(c) => c.decrypt_in_place_detached(nonce, &header, &mut body, tag),
        }
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "a message failed its RSA-AES check",
            )
        })?;
        Ok(body)
    }

    async fn send(&mut self, stream: &mut Box<dyn Stream>, plain: &[u8]) -> Result<(), Failure> {
        let message = self.seal(plain);
        stream.write_all(&message).await.map_err(lost)?;
        stream.flush().await.map_err(lost)
    }

    async fn receive(&mut self, stream: &mut Box<dyn Stream>) -> Result<Vec<u8>, Failure> {
        let mut header = [0; 2];
        stream.read_exact(&mut header).await.map_err(lost)?;
        let mut body = vec![0; usize::from(u16::from_be_bytes(header))];
        stream.read_exact(&mut body).await.map_err(lost)?;
        let mut tag = [0; 16];
        stream.read_exact(&mut tag).await.map_err(lost)?;
        self.open(header, body, &tag)
            .map_err(|e| Failure::Protocol(e.to_string()))
    }
}

/// The session's connection under RA2 and RA2_256.
struct Ra2Stream {
    inner: Box<dyn Stream>,
    channel: Channel,
    /// Bytes from the server not yet a whole message.
    raw: Vec<u8>,
    /// Decrypted bytes not yet read.
    plain: Vec<u8>,
    /// Sealed messages the connection hasn't taken yet.
    outgoing: Vec<u8>,
}

impl Ra2Stream {
    fn poll_drain(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while !self.outgoing.is_empty() {
            let written = ready!(Pin::new(&mut self.inner).poll_write(cx, &self.outgoing))?;
            if written == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
            self.outgoing.drain(..written);
        }
        Poll::Ready(Ok(()))
    }

    /// Decrypt the first message in `raw`, if it is all there.
    fn open_next(&mut self) -> io::Result<bool> {
        let Some(header) = self.raw.first_chunk::<2>().copied() else {
            return Ok(false);
        };
        let end = 2 + usize::from(u16::from_be_bytes(header)) + 16;
        if self.raw.len() < end {
            return Ok(false);
        }
        let body = self.raw[2..end - 16].to_vec();
        let plain = self.channel.open(header, body, &self.raw[end - 16..end])?;
        self.raw.drain(..end);
        self.plain.extend(plain);
        Ok(true)
    }
}

impl AsyncRead for Ra2Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            if !this.plain.is_empty() {
                let count = this.plain.len().min(buf.remaining());
                buf.put_slice(&this.plain[..count]);
                this.plain.drain(..count);
                return Poll::Ready(Ok(()));
            }
            if this.open_next()? {
                continue;
            }
            let mut bytes = [0; 16 << 10];
            let mut from = ReadBuf::new(&mut bytes);
            ready!(Pin::new(&mut this.inner).poll_read(cx, &mut from))?;
            if from.filled().is_empty() {
                return Poll::Ready(Ok(()));
            }
            this.raw.extend_from_slice(from.filled());
        }
    }
}

impl AsyncWrite for Ra2Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        ready!(this.poll_drain(cx))?;
        let taken = data.len().min(CHUNK);
        let message = this.channel.seal(&data[..taken]);
        this.outgoing.extend(message);
        if let Poll::Ready(Err(e)) = this.poll_drain(cx) {
            return Poll::Ready(Err(e));
        }
        Poll::Ready(Ok(taken))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_drain(cx))?;
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_drain(cx))?;
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}

/// A big-endian number, zero-filled on the left to `length` bytes.
fn padded(number: &[u8], length: usize) -> Vec<u8> {
    let mut out = vec![0; length.saturating_sub(number.len())];
    out.extend_from_slice(number);
    out
}

fn lost(error: io::Error) -> Failure {
    super::handshake::early(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_opens_on_the_other_side_once_and_in_order() {
        let key = [7; 16];
        let pair = || Channel {
            to_server: Cipher::Narrow(Eax::new(GenericArray::from_slice(&key))),
            from_server: Cipher::Narrow(Eax::new(GenericArray::from_slice(&key))),
            sent: 0,
            received: 0,
        };
        let (mut us, mut them) = (pair(), pair());
        let first = us.seal(b"hello");
        let second = us.seal(b"again");
        let open = |them: &mut Channel, message: &[u8]| {
            let header = [message[0], message[1]];
            let end = message.len() - 16;
            them.open(header, message[2..end].to_vec(), &message[end..])
        };
        assert_eq!(open(&mut them, &first).unwrap(), b"hello");
        assert_eq!(open(&mut them, &second).unwrap(), b"again");
        // Replayed, it carries the wrong count.
        assert!(open(&mut them, &first).is_err());
    }
}
