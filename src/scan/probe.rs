//! Asking one address whether it speaks RDP: the start of the connection
//! sequence, as far as the server's certificate. It goes no further, as
//! what comes next is signing in, and GNOME's Remote Login starts a display
//! at the login screen for a client once that succeeds. Servers expect a
//! sign-in after the certificate, so each logs a few errors as this leaves.
//! A VNC server speaks first: its greeting is all that is read.

use std::net::SocketAddrV4;
use std::time::Duration;

use ironrdp_pdu::nego::{ConnectionConfirm, ConnectionRequest, RequestFlags, SecurityProtocol};
use ironrdp_pdu::x224::X224;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use x509_cert::der::Encode as _;

/// How long an address gets to take the connection. Most have nothing there
/// and use all of it, so it is short; a computer that misses it is found
/// when the scan runs again.
pub(super) const CONNECT_FOR: Duration = Duration::from_millis(1500);

/// How long something that took the connection gets to answer like a server.
const ANSWER_FOR: Duration = Duration::from_secs(3);

/// How long the server gets to show its certificate.
const TLS_FOR: Duration = Duration::from_secs(3);

/// The longest Connection Confirm worth reading; real ones are 19 bytes.
const MAX_CONFIRM: usize = 256;

/// What a server said.
#[derive(Debug)]
pub(super) struct Answer {
    /// Its certificate, as DER, if it showed one.
    pub(super) certificate: Option<Vec<u8>>,
}

/// The answer of the server at `address`, or `None` when nothing there
/// speaks the protocol its port is for.
pub(super) async fn probe(address: SocketAddrV4) -> Option<Answer> {
    let mut stream = timeout(CONNECT_FOR, TcpStream::connect(address))
        .await
        .ok()?
        .ok()?;
    if super::protocol(address.port()) == crate::connection::Protocol::Vnc {
        let mut greeting = [0; 12];
        timeout(ANSWER_FOR, stream.read_exact(&mut greeting))
            .await
            .ok()?
            .ok()?;
        return greeting
            .starts_with(b"RFB ")
            .then_some(Answer { certificate: None });
    }
    let confirm = timeout(ANSWER_FOR, negotiate(&mut stream)).await.ok()??;
    let certificate = match confirm {
        ConnectionConfirm::Response { protocol, .. } if !protocol.is_standard_rdp_security() => {
            let host = address.ip().to_string();
            timeout(TLS_FOR, certificate(stream, &host))
                .await
                .ok()
                .flatten()
        }
        // It still speaks RDP, whatever it wouldn't agree to.
        _ => None,
    };
    Some(Answer { certificate })
}

/// Send a Connection Request, and read the Connection Confirm.
async fn negotiate(
    stream: &mut (impl AsyncRead + AsyncWrite + Unpin),
) -> Option<ConnectionConfirm> {
    let request = X224(ConnectionRequest {
        nego_data: None,
        flags: RequestFlags::empty(),
        protocol: SecurityProtocol::SSL | SecurityProtocol::HYBRID | SecurityProtocol::HYBRID_EX,
        correlation_info: None,
    });
    let request = ironrdp_core::encode_vec(&request).ok()?;
    stream.write_all(&request).await.ok()?;

    // A TPKT header: version 3, a reserved byte, and the length of it all.
    let mut header = [0; 4];
    stream.read_exact(&mut header).await.ok()?;
    let length = usize::from(u16::from_be_bytes([header[2], header[3]]));
    if header[0] != 3 || !(header.len()..=MAX_CONFIRM).contains(&length) {
        return None;
    }
    let mut pdu = vec![0; length];
    pdu[..header.len()].copy_from_slice(&header);
    stream.read_exact(&mut pdu[header.len()..]).await.ok()?;
    let X224(confirm) = ironrdp_core::decode::<X224<ConnectionConfirm>>(&pdu).ok()?;
    Some(confirm)
}

/// The TLS handshake, for the certificate. It is only read, so this needs
/// no opinion of it: the first session is what asks whether to trust it.
async fn certificate(stream: TcpStream, host: &str) -> Option<Vec<u8>> {
    crate::remote::use_ring();
    let (_tls, certificate) = ironrdp_tls::upgrade_with_certificate_validation(
        stream,
        host,
        ironrdp_tls::CertificateValidation::DangerouslyAcceptInvalidCertificate,
    )
    .await
    .ok()?;
    certificate.to_der().ok()
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr};

    use ironrdp_pdu::nego::{FailureCode, ResponseFlags};
    use tokio::net::TcpListener;

    use super::*;

    /// A server on a free port of this computer that answers the first thing
    /// it reads with `reply`.
    async fn server(reply: Vec<u8>) -> SocketAddrV4 {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let SocketAddr::V4(address) = listener.local_addr().unwrap() else {
            panic!("asked for IPv4");
        };
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 64];
            let _ = stream.read(&mut request).await;
            let _ = stream.write_all(&reply).await;
        });
        address
    }

    fn confirm(confirm: ConnectionConfirm) -> Vec<u8> {
        ironrdp_core::encode_vec(&X224(confirm)).unwrap()
    }

    /// A server on 127.0.0.1 at `port` that greets with `greeting`, or
    /// `None` when that port is taken.
    async fn greeter(port: u16, greeting: &'static [u8]) -> Option<SocketAddrV4> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await.ok()?;
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _ = stream.write_all(greeting).await;
        });
        Some(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))
    }

    #[tokio::test]
    async fn a_vnc_server_is_known_by_its_greeting_alone() {
        // On a VNC display's port, so the probe takes it for one; the
        // fourth and third are the least likely to be in use.
        let Some(vnc) = greeter(5903, b"RFB 003.008\n").await else {
            return;
        };
        assert!(probe(vnc).await.is_some_and(|a| a.certificate.is_none()));
        if let Some(other) = greeter(5902, b"SSH-2.0-OpenSSH\r\n").await {
            assert!(probe(other).await.is_none());
        }
    }

    #[tokio::test]
    async fn a_server_that_refuses_the_security_still_counts() {
        let reply = confirm(ConnectionConfirm::Failure {
            code: FailureCode::SSL_NOT_ALLOWED_BY_SERVER,
        });
        let answer = probe(server(reply).await).await.expect("an RDP server");
        assert!(answer.certificate.is_none());
    }

    #[tokio::test]
    async fn a_server_with_old_security_has_no_certificate_to_show() {
        let reply = confirm(ConnectionConfirm::Response {
            flags: ResponseFlags::empty(),
            protocol: SecurityProtocol::empty(),
        });
        let answer = probe(server(reply).await).await.expect("an RDP server");
        assert!(answer.certificate.is_none());
    }

    #[tokio::test]
    async fn something_else_on_the_port_does_not_count() {
        for reply in [
            b"SSH-2.0-OpenSSH_9.9\r\n".to_vec(),
            b"HTTP/1.1 400 Bad Request\r\n\r\n".to_vec(),
            // A TPKT header that promises more than a confirm could be.
            vec![3, 0, 0xFF, 0xFF, 0, 0, 0, 0],
            // A TPDU that isn't a confirm.
            vec![3, 0, 0, 11, 6, 0xE0, 0, 0, 0, 0, 0],
            Vec::new(),
        ] {
            assert!(
                probe(server(reply.clone()).await).await.is_none(),
                "{reply:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_closed_port_does_not_count() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let SocketAddr::V4(address) = listener.local_addr().unwrap() else {
            panic!("asked for IPv4");
        };
        drop(listener);
        assert!(probe(address).await.is_none());
    }

    /// The probe against a real server, which `temp/grd/run.sh` is:
    /// `MOTE_TEST_PROBE=127.0.0.1:3399 cargo test -- --ignored live_probe`.
    #[tokio::test]
    #[ignore]
    async fn live_probe() {
        let address = std::env::var("MOTE_TEST_PROBE").expect("MOTE_TEST_PROBE");
        let answer = probe(address.parse().expect("address:port"))
            .await
            .expect("an RDP server");
        let der = answer.certificate.expect("a certificate");
        eprintln!(
            "server {:?}, name {:?}",
            crate::rdp::server::Server::from_certificate(&der),
            crate::rdp::server::certificate_name(&der),
        );
    }
}
