//! Apple's Diffie-Hellman sign-in (security type 30), which wayvnc offers
//! when told to relax its encryption, as macOS does: the user name and
//! password go encrypted with a key both sides agree on, and the session
//! after them goes as it is, but for macOS, which encrypts it after with a
//! key wrapped in that one.

use md5::{Digest, Md5};
use rsa::BigUint;
use rsa::rand_core::{OsRng, RngCore};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use aes::Aes128;
use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockEncrypt, KeyInit};

use super::handshake::early;
use crate::remote::Failure;
use crate::remote::route::Stream;

pub(super) const APPLE_DH: u8 = 30;

/// Each of the user name and the password takes this many bytes.
const FIELD: usize = 64;

/// Sign in, and return the key agreed on, which macOS wraps the session's
/// first key in.
pub(super) async fn sign_in(
    stream: &mut Box<dyn Stream>,
    username: &str,
    password: &str,
) -> Result<[u8; 16], Failure> {
    let generator = stream.read_u16().await.map_err(early)?;
    let size = usize::from(stream.read_u16().await.map_err(early)?);
    if !(16..=1024).contains(&size) {
        return Err(Failure::Protocol(format!(
            "a Diffie-Hellman key of {size} bytes"
        )));
    }
    let mut numbers = vec![0; size * 2];
    stream.read_exact(&mut numbers).await.map_err(early)?;
    let (prime, theirs) = numbers.split_at(size);
    let prime = BigUint::from_bytes_be(prime);
    let theirs = BigUint::from_bytes_be(theirs);

    let mut secret = vec![0; size];
    OsRng.fill_bytes(&mut secret);
    let secret = BigUint::from_bytes_be(&secret);
    let ours = BigUint::from(generator).modpow(&secret, &prime);
    let shared = theirs.modpow(&secret, &prime);
    let key = Md5::digest(fixed(&shared.to_bytes_be(), size));

    let mut credentials = credentials(username, password)?;
    let cipher = Aes128::new(&key);
    for block in credentials.chunks_mut(16) {
        cipher.encrypt_block(GenericArray::from_mut_slice(block));
    }
    let mut message = credentials.to_vec();
    message.extend(fixed(&ours.to_bytes_be(), size));
    stream.write_all(&message).await.map_err(early)?;
    stream.flush().await.map_err(early)?;
    Ok(key.into())
}

/// The user name and password as Apple's sign-ins take them, before they
/// are encrypted: 64 bytes each, NUL-terminated, the rest random.
pub(super) fn credentials(username: &str, password: &str) -> Result<[u8; FIELD * 2], Failure> {
    let mut credentials = [0; FIELD * 2];
    OsRng.fill_bytes(&mut credentials);
    for (field, text) in credentials.chunks_mut(FIELD).zip([username, password]) {
        let text = text.as_bytes();
        if text.len() >= FIELD {
            return Err(Failure::Protocol(
                "a user name or password of 64 bytes or more".into(),
            ));
        }
        field[..text.len()].copy_from_slice(text);
        field[text.len()] = 0;
    }
    Ok(credentials)
}

/// A big-endian number in exactly `size` bytes.
fn fixed(number: &[u8], size: usize) -> Vec<u8> {
    let mut out = vec![0; size.saturating_sub(number.len())];
    out.extend_from_slice(&number[number.len().saturating_sub(size)..]);
    out
}
