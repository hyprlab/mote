//! Apple's SRP, the newest way into macOS Screen Sharing (security type
//! 33): the user name under the server's RSA key, then SRP-6a over RFC
//! 3526's 4096-bit group with SHA-512, the password stretched with PBKDF2.
//! Each side proves it knows the password, so a server that answers with
//! the right proof holds the account's verifier, which is what verifies it.

use rsa::rand_core::{OsRng, RngCore};
use rsa::{BigUint, Pkcs1v15Encrypt};
use sha2::{Digest, Sha256, Sha512};
use zeroize::Zeroizing;

use super::{ServerKey, envelope, framed, message, send};
use crate::remote::Failure;
use crate::remote::route::Stream;

/// RFC 3526's 4096-bit prime, which macOS uses with a generator of 5. A
/// server offering any other group is turned away: SRP is only as strong
/// as its group.
const PRIME: &str = "\
    FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74\
    020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F1437\
    4FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7ED\
    EE386BFB5A899FA5AE9F24117C4B1FE649286651ECE45B3DC2007CB8A163BF05\
    98DA48361C55D39A69163FA8FD24CF5F83655D23DCA3AD961C62F356208552BB\
    9ED529077096966D670C354E4ABC9804F1746C08CA18217C32905E462E36CE3B\
    E39E772C180E86039B2783A2EC07A28FB5C55DF06F4C52C9DE2BCBF695581718\
    3995497CEA956AE515D2261898FA051015728E5A8AAAC42DAD33170D04507A33\
    A85521ABDF1CBA64ECFB850458DBEF0A8AEA71575D060C7DB3970F85A6E1E4C7\
    ABF5AE8CDB0933D71E8C94E04A25619DCEE3D2261AD2EE6BF12FFA06D98A0864\
    D87602733EC86A64521F2B18177B200CBBE117577A615D6C770988C0BAD946E2\
    08E24FA074E5AB3143DB5BFCE0FD108E4B82D120A92108011A723C12A787E6D7\
    88719A10BDBA5B2699C327186AF4E23C1A946834B6150BDA2583E9CA2AD44CE8\
    DBBBC2DB04DE8EF92E8EFC141FBECAA6287C59474E6BC05D99B2964FA090C3A2\
    233BA186515BE7ED1F612970CEE2D7AFB81BDD762170481CD0069127D5B05AA9\
    93B4EA988D8FDDC186FFB7DC90A6C08F4DF435C934063199FFFFFFFFFFFFFFFF";
const GENERATOR: u32 = 5;
/// The group's size in bytes, to which its numbers are padded.
const SIZE: usize = 512;

/// The most PBKDF2 rounds taken: macOS asks for around 100,000, and a
/// server asking for far more would only keep this side busy.
const MAX_ROUNDS: u64 = 1_000_000;
/// The framing macOS gives the second message, padded to this length.
const PROOF_LENGTH: usize = 1076;

/// Sign in as `username`, once the server's key has come, and return the
/// key the session's first key comes wrapped in. The server's verdict
/// follows, read as every type's is.
pub(in crate::vnc) async fn sign_in(
    stream: &mut Box<dyn Stream>,
    key: &ServerKey,
    username: &str,
    password: &str,
) -> Result<[u8; 16], Failure> {
    // The user name, under that key.
    let mut identity = (username.len() as u32).to_be_bytes().to_vec();
    identity.extend_from_slice(username.as_bytes());
    identity.extend_from_slice(&[0, 0, 0]);
    let mut plain = (identity.len() as u32).to_be_bytes().to_vec();
    plain.extend_from_slice(&identity);
    let sealed = key
        .key
        .encrypt(&mut OsRng, Pkcs1v15Encrypt, &plain)
        .map_err(|e| Failure::Protocol(format!("the server's RSA key: {e}")))?;
    let mut first = envelope(2).to_vec();
    first.extend_from_slice(&(sealed.len() as u16).to_be_bytes());
    first.extend_from_slice(&sealed);
    first.resize(first.len() + 384, 0);
    send(stream, &framed(first)).await?;

    let challenge = Challenge::read(&message(stream).await?)?;
    tracing::debug!("Apple SRP: {} rounds of PBKDF2", challenge.rounds);
    let password = Zeroizing::new(password.to_owned());
    let proof = tokio::task::spawn_blocking(move || prove(&challenge, &password))
        .await
        .map_err(|e| Failure::Protocol(e.to_string()))?;

    let mut inner = (SIZE as u16).to_be_bytes().to_vec();
    inner.extend_from_slice(&proof.public);
    inner.push(proof.m1.len() as u8);
    inner.extend_from_slice(&proof.m1);
    inner.extend_from_slice(&(proof.options.len() as u16).to_be_bytes());
    inner.extend_from_slice(&proof.options);
    let mut nonce = [0; 16];
    OsRng.fill_bytes(&mut nonce);
    inner.push(nonce.len() as u8);
    inner.extend_from_slice(&nonce);
    let mut second = envelope(2).to_vec();
    second.extend_from_slice(&(inner.len() as u16 + 4).to_be_bytes());
    second.extend_from_slice(&(inner.len() as u32).to_be_bytes());
    second.extend_from_slice(&inner);
    if second.len() < PROOF_LENGTH {
        second.resize(PROOF_LENGTH, 0);
    }
    send(stream, &framed(second)).await?;

    // The server's own proof, after a header of 10 bytes. A wrong password
    // gets a verdict of failure instead, in a message too short for one.
    let reply = message(stream).await?;
    let theirs = reply
        .get(10)
        .and_then(|&length| reply.get(11..11 + usize::from(length)));
    match theirs {
        Some(m2) if m2 == proof.m2.as_slice() => {
            Ok(Sha256::digest(*proof.key)[..16].try_into().unwrap())
        }
        Some(_) => Err(Failure::Protocol(
            "the server's proof of the password is wrong".into(),
        )),
        None => Err(Failure::Refused),
    }
}

/// What the server sends to start SRP.
#[derive(Debug)]
struct Challenge {
    salt: Vec<u8>,
    /// Its public number, padded to the group's size.
    public: Vec<u8>,
    rounds: u32,
    /// The options it uses, which go back to it as they came.
    options: Vec<u8>,
}

impl Challenge {
    /// After 10 bytes of header: a byte, the prime, the generator, the
    /// salt, the server's number, the rounds and the options, each but the
    /// rounds with its length first.
    fn read(bytes: &[u8]) -> Result<Self, Failure> {
        let bad = |what: &str| Failure::Protocol(format!("the SRP challenge's {what}"));
        let mut at = Cursor { bytes, at: 10 };
        at.take(1).ok_or_else(|| bad("start"))?;
        let prime = at.long().ok_or_else(|| bad("prime"))?;
        let generator = at.long().ok_or_else(|| bad("generator"))?;
        let salt = at.short().ok_or_else(|| bad("salt"))?;
        let public = at.long().ok_or_else(|| bad("number"))?;
        let rounds = at.take(8).ok_or_else(|| bad("rounds"))?;
        let rounds = u64::from_be_bytes(rounds.try_into().unwrap());
        let options = at.long().ok_or_else(|| bad("options"))?;

        if BigUint::from_bytes_be(prime) != group() {
            return Err(Failure::Protocol("an SRP group other than macOS's".into()));
        }
        if BigUint::from_bytes_be(generator) != BigUint::from(GENERATOR) {
            return Err(Failure::Protocol("an SRP generator other than 5".into()));
        }
        let number = BigUint::from_bytes_be(public);
        if number.clone() % group() == BigUint::default() {
            return Err(Failure::Protocol("an SRP number of zero".into()));
        }
        if rounds == 0 || rounds > MAX_ROUNDS {
            return Err(Failure::Protocol(format!("{rounds} rounds of PBKDF2")));
        }
        Ok(Self {
            salt: salt.to_vec(),
            public: pad(&number),
            rounds: rounds as u32,
            options: options.to_vec(),
        })
    }
}

/// Reads the sign-in's fields in turn.
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, length: usize) -> Option<&'a [u8]> {
        let taken = self.bytes.get(self.at..self.at.checked_add(length)?)?;
        self.at += length;
        Some(taken)
    }

    /// A field with a length of one byte first.
    fn short(&mut self) -> Option<&'a [u8]> {
        let length = self.take(1)?[0];
        self.take(length.into())
    }

    /// A field with a length of two bytes first.
    fn long(&mut self) -> Option<&'a [u8]> {
        let length = self.take(2)?;
        self.take(u16::from_be_bytes([length[0], length[1]]).into())
    }
}

fn group() -> BigUint {
    BigUint::parse_bytes(PRIME.as_bytes(), 16).expect("the prime is hexadecimal")
}

/// A number of the group, padded to its size.
fn pad(number: &BigUint) -> Vec<u8> {
    let bytes = number.to_bytes_be();
    let mut out = vec![0; SIZE.saturating_sub(bytes.len())];
    out.extend_from_slice(&bytes);
    out
}

fn hash(parts: &[&[u8]]) -> [u8; 64] {
    let mut hasher = Sha512::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

/// This side's half of SRP.
struct Proof {
    /// The client's public number, padded.
    public: Vec<u8>,
    m1: [u8; 64],
    /// The proof the server should answer with.
    m2: [u8; 64],
    /// The shared key.
    key: Zeroizing<[u8; 64]>,
    options: Vec<u8>,
}

/// SRP-6a as macOS runs it: the password stretched with PBKDF2 into 128
/// bytes, and an empty user name in the hashes, as the name went under RSA.
fn prove(challenge: &Challenge, password: &str) -> Proof {
    let mut secret = [0; 64];
    OsRng.fill_bytes(&mut secret);
    prove_with(challenge, password, &secret)
}

fn prove_with(challenge: &Challenge, password: &str, secret: &[u8]) -> Proof {
    let n = group();
    let g = BigUint::from(GENERATOR);
    let (n_bytes, g_bytes) = (pad(&n), pad(&g));
    let k = BigUint::from_bytes_be(&hash(&[&n_bytes, &g_bytes]));
    let a = BigUint::from_bytes_be(secret);
    let public = pad(&g.modpow(&a, &n));
    let u = BigUint::from_bytes_be(&hash(&[&public, &challenge.public]));

    let mut stretched = Zeroizing::new([0; 128]);
    pbkdf2::pbkdf2_hmac::<Sha512>(
        password.as_bytes(),
        &challenge.salt,
        challenge.rounds,
        stretched.as_mut(),
    );
    let inner = Zeroizing::new(hash(&[b":", stretched.as_ref()]));
    let x = BigUint::from_bytes_be(&hash(&[&challenge.salt, inner.as_ref()]));

    // S = (B - k·g^x)^(a + u·x), all mod N.
    let b = BigUint::from_bytes_be(&challenge.public);
    let kgx = (k * g.modpow(&x, &n)) % &n;
    let base = (b + &n - kgx) % &n;
    let shared = base.modpow(&(a + u * x), &n);
    let key = Zeroizing::new(hash(&[&pad(&shared)]));

    let (hn, hg) = (hash(&[&n_bytes]), hash(&[&g_bytes]));
    let mixed: Vec<u8> = hn.iter().zip(hg).map(|(n, g)| n ^ g).collect();
    let m1 = hash(&[
        &mixed,
        &hash(&[b""]),
        &challenge.salt,
        &public,
        &challenge.public,
        key.as_ref(),
    ]);
    let m2 = hash(&[&public, &m1, key.as_ref()]);
    Proof {
        public,
        m1,
        m2,
        key,
        options: challenge.options.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The server's side of SRP-6a, to check this side's against: the
    /// verifier from the password, and the shared key from `A`.
    fn server(password: &str, salt: &[u8], rounds: u32, b: &BigUint) -> (BigUint, Vec<u8>) {
        let n = group();
        let g = BigUint::from(GENERATOR);
        let mut stretched = [0; 128];
        pbkdf2::pbkdf2_hmac::<Sha512>(password.as_bytes(), salt, rounds, &mut stretched);
        let x = BigUint::from_bytes_be(&hash(&[salt, &hash(&[b":", &stretched])]));
        let v = g.modpow(&x, &n);
        let k = BigUint::from_bytes_be(&hash(&[&pad(&n), &pad(&g)]));
        let public = (k * &v + g.modpow(b, &n)) % &n;
        (v, pad(&public))
    }

    #[test]
    fn both_sides_reach_the_same_key_and_proofs() {
        let salt = [7; 32];
        let b = BigUint::from_bytes_be(&[0x5a; 64]);
        let (v, public) = server("hunter2", &salt, 1000, &b);
        let challenge = Challenge {
            salt: salt.to_vec(),
            public: public.clone(),
            rounds: 1000,
            options: b"mda=SHA-512".to_vec(),
        };
        let proof = prove_with(&challenge, "hunter2", &[0x3c; 64]);

        // The server's S = (A·v^u)^b.
        let n = group();
        let a = BigUint::from_bytes_be(&proof.public);
        let u = BigUint::from_bytes_be(&hash(&[&proof.public, &public]));
        let shared = (a * v.modpow(&u, &n)).modpow(&b, &n);
        let key = hash(&[&pad(&shared)]);
        assert_eq!(*proof.key, key);
        assert_eq!(proof.m2, hash(&[&proof.public, &proof.m1, &key]));

        // A wrong password reaches another key.
        let wrong = prove_with(&challenge, "hunter3", &[0x3c; 64]);
        assert_ne!(*wrong.key, key);
    }

    #[test]
    fn reads_the_challenge_as_macos_frames_it() {
        let mut body = vec![0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0];
        let field = |body: &mut Vec<u8>, bytes: &[u8]| {
            body.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
            body.extend_from_slice(bytes);
        };
        field(&mut body, &pad(&group()));
        field(&mut body, &[5]);
        body.push(32);
        body.extend_from_slice(&[1; 32]);
        field(&mut body, &[9; SIZE]);
        body.extend_from_slice(&116_279u64.to_be_bytes());
        field(&mut body, b"mda=SHA-512,kdf=SALTED-SHA512-PBKDF2");
        let Ok(challenge) = Challenge::read(&body) else {
            panic!("the challenge should read");
        };
        assert_eq!(challenge.rounds, 116_279);
        assert_eq!(challenge.salt, [1; 32]);
        assert_eq!(challenge.options, b"mda=SHA-512,kdf=SALTED-SHA512-PBKDF2");

        // Another group is turned away.
        let mut weak = body.clone();
        weak[20] ^= 1;
        assert!(Challenge::read(&weak).is_err());
        assert!(Challenge::read(&body[..600]).is_err());
    }
}
