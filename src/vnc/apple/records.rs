//! The encryption macOS Screen Sharing puts around a session after its
//! sign-in: AES-128 in CBC mode, one chain each way for the whole session,
//! in records that each end with a SHA-1 of a sequence number and their
//! contents. The server sends the first key and IV wrapped under the key
//! the sign-in agreed on, once the client asks for encryption, and may
//! send another the same way later.

use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, ready};
use std::time::Duration;

use aes::Aes128;
use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use sha1::{Digest, Sha1};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

use super::super::handshake::early;
use crate::remote::Failure;
use crate::remote::route::Stream;

/// A new key from the server, as its update carries it: a generation, then
/// the key and the IV, each wrapped.
pub(in crate::vnc) type Rekey = [u8; 36];

/// Keys the session's reader has read, for the records to change to before
/// the next one. Shared, as the reader sees them and the stream uses them.
pub(in crate::vnc) type Rekeys = Arc<Mutex<Vec<Rekey>>>;

/// The update rectangle's encoding that carries a key.
pub(in crate::vnc) const ENCRYPTION_INFO: i32 = 0x44f;
/// macOS's message of status, which the client is sent unasked.
pub(in crate::vnc) const MISC_STATUS: u8 = 0x14;

/// How long the server gets to send the first key.
const KEY_WITHIN: Duration = Duration::from_secs(10);
/// The integrity check at each record's end.
const CHECK: usize = 20;
/// The most a record carries: what macOS sends at most itself.
const MOST: usize = 32768;

/// What Screen Sharing.app tells the server of itself: a viewer of version
/// 6.1 on macOS 15.3, and the features it takes, including the status
/// messages and clipboard notices.
fn viewer_info() -> Vec<u8> {
    let mut out = vec![0x21, 0, 0, 0x3e, 0, 1];
    for number in [2u32, 6, 1, 0, 15, 3, 0] {
        out.extend_from_slice(&number.to_be_bytes());
    }
    let mut features = [0; 32];
    features[0] = 0xb0;
    features[2] = 0x0c;
    features[3] = 0x03;
    features[4] = 0x90;
    features[10] = 0x40;
    out.extend_from_slice(&features);
    out
}

/// SetEncryption: start with AES-128, the one method there is.
const START: [u8; 12] = [0x12, 0, 0, 1, 0, 1, 0, 1, 0, 0, 0, 1];
/// SetEncryption's second command, after which everything is in records.
const STARTED: [u8; 8] = [0x12, 0, 0, 2, 0, 1, 0, 0];

/// Ask for encryption, and put the stream in records once the server sends
/// the key. `wrap` is the key from the sign-in.
pub(in crate::vnc) async fn start(
    mut stream: Box<dyn Stream>,
    wrap: [u8; 16],
) -> Result<(Records, Rekeys), Failure> {
    let mut hello = viewer_info();
    hello.extend_from_slice(&START);
    stream.write_all(&hello).await.map_err(early)?;
    stream.flush().await.map_err(early)?;
    let rekey = tokio::time::timeout(KEY_WITHIN, first_key(&mut stream))
        .await
        .map_err(|_| Failure::Protocol("the server sent no session key".into()))??;
    stream.write_all(&STARTED).await.map_err(early)?;
    stream.flush().await.map_err(early)?;
    let rekeys = Rekeys::default();
    let records = Records::new(stream, wrap, &rekey, rekeys.clone());
    tracing::info!("the session is in Apple's records");
    Ok((records, rekeys))
}

/// Read up to the update that carries the first key, past status messages.
async fn first_key(stream: &mut Box<dyn Stream>) -> Result<Rekey, Failure> {
    loop {
        match stream.read_u8().await.map_err(early)? {
            MISC_STATUS => {
                stream.read_u8().await.map_err(early)?;
                let length = stream.read_u16().await.map_err(early)?;
                let mut skipped = vec![0; usize::from(length)];
                stream.read_exact(&mut skipped).await.map_err(early)?;
            }
            0 => {
                stream.read_u8().await.map_err(early)?;
                if stream.read_u16().await.map_err(early)? == 0 {
                    continue;
                }
                let mut header = [0; 12];
                stream.read_exact(&mut header).await.map_err(early)?;
                let kind = i32::from_be_bytes(header[8..].try_into().unwrap());
                if kind != ENCRYPTION_INFO {
                    return Err(Failure::Protocol(format!(
                        "encoding {kind} before the session key"
                    )));
                }
                let mut rekey = [0; 36];
                stream.read_exact(&mut rekey).await.map_err(early)?;
                return Ok(rekey);
            }
            other => {
                return Err(Failure::Protocol(format!(
                    "message type {other} before the session key"
                )));
            }
        }
    }
}

/// One direction's chain.
struct Chain {
    cipher: Aes128,
    /// The last block, which the next is chained to.
    last: [u8; 16],
    sequence: u32,
}

impl Chain {
    fn encrypt(&mut self, data: &mut [u8]) {
        for block in data.as_chunks_mut::<16>().0 {
            for (byte, last) in block.iter_mut().zip(self.last) {
                *byte ^= last;
            }
            self.cipher
                .encrypt_block(GenericArray::from_mut_slice(block));
            self.last = *block;
        }
    }

    fn decrypt(&mut self, data: &mut [u8]) {
        for block in data.as_chunks_mut::<16>().0 {
            let sealed = *block;
            self.cipher
                .decrypt_block(GenericArray::from_mut_slice(block));
            for (byte, last) in block.iter_mut().zip(self.last) {
                *byte ^= last;
            }
            self.last = sealed;
        }
    }
}

/// The check of a record's contents, under its sequence number.
fn check(sequence: u32, contents: &[u8]) -> [u8; CHECK] {
    let mut hasher = Sha1::new();
    hasher.update(sequence.to_be_bytes());
    hasher.update(contents);
    hasher.finalize().into()
}

/// A stream in Apple's records. Each message written goes in a record of
/// its own once flushed, as macOS reads one message a record.
pub(in crate::vnc) struct Records {
    inner: Box<dyn Stream>,
    /// The key the next key comes wrapped in.
    wrap: Aes128,
    send: Chain,
    receive: Chain,
    rekeys: Rekeys,
    /// Read and not yet a whole record.
    sealed_in: Vec<u8>,
    /// The current record's contents, and how far they have been read.
    plain: Vec<u8>,
    read: usize,
    /// Written and not yet flushed.
    pending: Vec<u8>,
    /// Records made and not yet all sent.
    sealed_out: Vec<u8>,
    sent: usize,
}

impl Records {
    fn new(inner: Box<dyn Stream>, wrap: [u8; 16], rekey: &Rekey, rekeys: Rekeys) -> Self {
        let chain = || Chain {
            cipher: Aes128::new(&[0; 16].into()),
            last: [0; 16],
            sequence: 0,
        };
        let mut records = Self {
            inner,
            wrap: Aes128::new(&wrap.into()),
            send: chain(),
            receive: chain(),
            rekeys,
            sealed_in: Vec::new(),
            plain: Vec::new(),
            read: 0,
            pending: Vec::new(),
            sealed_out: Vec::new(),
            sent: 0,
        };
        records.rekey(rekey);
        records
    }

    /// Change to the key in `rekey`: both ways, with the same IV, and the
    /// key the next comes wrapped in. The sequence numbers go on.
    fn rekey(&mut self, rekey: &Rekey) {
        let mut key = GenericArray::clone_from_slice(&rekey[4..20]);
        let mut iv = GenericArray::clone_from_slice(&rekey[20..36]);
        self.wrap.decrypt_block(&mut key);
        self.wrap.decrypt_block(&mut iv);
        for chain in [&mut self.send, &mut self.receive] {
            chain.cipher = Aes128::new(&key);
            chain.last = iv.into();
        }
        self.wrap = Aes128::new(&key);
    }

    fn take_rekeys(&mut self) {
        let waiting = std::mem::take(&mut *self.rekeys.lock().unwrap());
        for rekey in &waiting {
            tracing::info!("the server changed the session key");
            self.rekey(rekey);
        }
    }

    /// Open the record at the start of `sealed_in`, once it is all there.
    /// `Ok(false)` while it isn't.
    fn open(&mut self) -> io::Result<bool> {
        let Some(head) = self.sealed_in.get(..2) else {
            return Ok(false);
        };
        let length = usize::from(u16::from_be_bytes([head[0], head[1]]));
        if length < 2 + CHECK || length % 16 != 0 {
            return Err(bad(format!("a record of {length} bytes")));
        }
        if self.sealed_in.len() < 2 + length {
            return Ok(false);
        }
        self.take_rekeys();
        let mut record: Vec<u8> = self.sealed_in.drain(..2 + length).skip(2).collect();
        self.receive.decrypt(&mut record);
        let (contents, sum) = record.split_at(length - CHECK);
        if check(self.receive.sequence, contents) != sum {
            return Err(bad("a record that fails its check".into()));
        }
        self.receive.sequence = self.receive.sequence.wrapping_add(1);
        let body = usize::from(u16::from_be_bytes([contents[0], contents[1]]));
        if 2 + body > contents.len() {
            return Err(bad(format!("a record that claims {body} bytes")));
        }
        record.truncate(2 + body);
        record.drain(..2);
        self.plain = record;
        self.read = 0;
        Ok(true)
    }

    /// Put what was written in records, one message to each.
    fn seal(&mut self) {
        self.take_rekeys();
        let pending = std::mem::take(&mut self.pending);
        let mut rest = pending.as_slice();
        while !rest.is_empty() {
            let length = message_length(rest).min(rest.len());
            let (message, after) = rest.split_at(length);
            for body in message.chunks(MOST) {
                self.seal_one(body);
            }
            rest = after;
        }
    }

    fn seal_one(&mut self, body: &[u8]) {
        let filler = (16 - (2 + body.len() + CHECK) % 16) % 16;
        let mut record = Vec::with_capacity(2 + body.len() + filler + CHECK);
        record.extend_from_slice(&(body.len() as u16).to_be_bytes());
        record.extend_from_slice(body);
        record.resize(record.len() + filler, 0);
        let sum = check(self.send.sequence, &record);
        record.extend_from_slice(&sum);
        self.send.sequence = self.send.sequence.wrapping_add(1);
        self.send.encrypt(&mut record);
        self.sealed_out
            .extend_from_slice(&(record.len() as u16).to_be_bytes());
        self.sealed_out.extend_from_slice(&record);
    }
}

fn bad(why: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why)
}

/// How long the client message at the start of `bytes` is, or all of
/// `bytes` for one this doesn't know.
fn message_length(bytes: &[u8]) -> usize {
    let u16_at = |at: usize| {
        bytes
            .get(at..at + 2)
            .map_or(0, |b| usize::from(u16::from_be_bytes([b[0], b[1]])))
    };
    match bytes[0] {
        // SetPixelFormat, SetEncodings, FramebufferUpdateRequest, KeyEvent
        // and PointerEvent.
        0 => 20,
        2 => 4 + 4 * u16_at(2),
        3 => 10,
        4 => 8,
        5 => 6,
        // ClientCutText, extended when its length is negative.
        6 => bytes.get(4..8).map_or(bytes.len(), |b| {
            8 + i32::from_be_bytes(b.try_into().unwrap()).unsigned_abs() as usize
        }),
        // QEMU's extended key event.
        255 if bytes.get(1) == Some(&0) => 12,
        // macOS's: watching its clipboard and asking for it, and putting
        // something in it.
        0x0b | 0x15 => 8,
        0x1f => bytes.get(12..16).map_or(bytes.len(), |b| {
            16 + u32::from_be_bytes(b.try_into().unwrap()) as usize
        }),
        _ => bytes.len(),
    }
}

impl AsyncRead for Records {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            if this.read < this.plain.len() {
                let available = &this.plain[this.read..];
                let taken = available.len().min(buf.remaining());
                buf.put_slice(&available[..taken]);
                this.read += taken;
                return Poll::Ready(Ok(()));
            }
            if this.open()? {
                continue;
            }
            let mut chunk = [0; 16 << 10];
            let mut read = ReadBuf::new(&mut chunk);
            ready!(Pin::new(&mut this.inner).poll_read(cx, &mut read))?;
            if read.filled().is_empty() {
                // The end, which is only clean between records.
                if this.sealed_in.is_empty() {
                    return Poll::Ready(Ok(()));
                }
                return Poll::Ready(Err(io::ErrorKind::UnexpectedEof.into()));
            }
            this.sealed_in.extend_from_slice(read.filled());
        }
    }
}

impl AsyncWrite for Records {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.get_mut().pending.extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.sent == this.sealed_out.len() && !this.pending.is_empty() {
            this.sealed_out.clear();
            this.sent = 0;
            this.seal();
        }
        while this.sent < this.sealed_out.len() {
            let written =
                ready!(Pin::new(&mut this.inner).poll_write(cx, &this.sealed_out[this.sent..]))?;
            if written == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
            this.sent += written;
        }
        if !this.pending.is_empty() {
            // Written while the last records went out.
            return Pin::new(this).poll_flush(cx);
        }
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(Pin::new(&mut *this).poll_flush(cx))?;
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records from one side read on the other: two ends of a pipe, one
    /// with its directions swapped, as the server's are.
    #[tokio::test]
    async fn records_carry_messages_both_ways_and_across_a_new_key() {
        let wrap = [9; 16];
        let wrapped = |key: [u8; 16], iv: [u8; 16], under: [u8; 16]| {
            let cipher = Aes128::new(&under.into());
            let (mut key, mut iv) = (GenericArray::from(key), GenericArray::from(iv));
            cipher.encrypt_block(&mut key);
            cipher.encrypt_block(&mut iv);
            let mut rekey = [0; 36];
            rekey[3] = 1;
            rekey[4..20].copy_from_slice(&key);
            rekey[20..].copy_from_slice(&iv);
            rekey
        };
        let first = wrapped([1; 16], [2; 16], wrap);
        let (near, far) = tokio::io::duplex(1 << 20);
        let mut client = Records::new(Box::new(near), wrap, &first, Rekeys::default());
        let mut server = Records::new(Box::new(far), wrap, &first, Rekeys::default());

        // Two messages in one write go as two records.
        let mut both = vec![3, 0, 0, 0, 0, 0, 1, 0, 1, 0];
        both.extend_from_slice(&[4, 1, 0, 0, 0, 0, 0, 0x61]);
        client.write_all(&both).await.unwrap();
        client.flush().await.unwrap();
        let mut got = vec![0; both.len()];
        server.read_exact(&mut got).await.unwrap();
        assert_eq!(got, both);
        assert_eq!(client.send.sequence, 2);

        // A long message, past one record, the other way.
        let long: Vec<u8> = (0..100_000u32).map(|i| i as u8).collect();
        server.write_all(&long).await.unwrap();
        server.flush().await.unwrap();
        let mut got = vec![0; long.len()];
        client.read_exact(&mut got).await.unwrap();
        assert_eq!(got, long);

        // A new key, wrapped under the last, as the reader would pass it on.
        let next = wrapped([5; 16], [6; 16], [1; 16]);
        client.rekeys.lock().unwrap().push(next);
        server.rekey(&next);
        server.write_all(b"after").await.unwrap();
        server.flush().await.unwrap();
        let mut got = [0; 5];
        client.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"after");
    }

    #[tokio::test]
    async fn a_record_altered_on_the_way_is_refused() {
        let rekey = [3; 36];
        let (near, mut far) = tokio::io::duplex(4096);
        let mut client = Records::new(Box::new(near), [4; 16], &rekey, Rekeys::default());
        let (other, _kept) = tokio::io::duplex(64);
        let mut server = Records::new(Box::new(other), [4; 16], &rekey, Rekeys::default());
        server.pending.extend_from_slice(&[2, 0, 0, 1, 0, 0, 0, 16]);
        server.seal();
        let mut sealed = server.sealed_out.clone();
        sealed[10] ^= 1;
        far.write_all(&sealed).await.unwrap();
        let mut got = [0; 8];
        assert!(client.read_exact(&mut got).await.is_err());
    }

    #[test]
    fn knows_the_lengths_of_the_messages_it_splits() {
        assert_eq!(message_length(&[2, 0, 0, 3, 0, 0, 0, 0]), 16);
        assert_eq!(message_length(&[6, 0, 0, 0, 0xff, 0xff, 0xff, 0xfc]), 12);
        assert_eq!(message_length(&[6, 0, 0, 0, 0, 0, 0, 5]), 13);
        assert_eq!(message_length(&[200, 1, 2]), 3);
    }
}
