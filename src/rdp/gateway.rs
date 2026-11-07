//! An RD Gateway (MS-TSGU): the connection goes to the gateway over HTTPS,
//! and the gateway passes it on to the computer.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use ironrdp_mstsgu::{GwClient, GwConnectTarget, GwErrorKind};
use ironrdp_tls::{CertificateValidation, CertificateValidationCallback};
use tokio::sync::{mpsc, oneshot};

use super::{
    Event, Failure, Hop, Identity, Target, Trust, Tunnel, Unreachable, describe, io_error,
};
use crate::remote::fingerprint;
use crate::remote::route::Stream;

pub(crate) async fn open(
    hop: &Hop,
    target: &Target,
    events: &mpsc::Sender<Event>,
) -> Result<(Box<dyn Stream>, SocketAddr), Failure> {
    let endpoint = if hop.host.contains(':') {
        format!("[{}]:{}", hop.host, hop.port)
    } else {
        format!("{}:{}", hop.host, hop.port)
    };
    let gateway = GwConnectTarget {
        gw_endpoint: endpoint,
        gw_user: hop.username.clone(),
        gw_pass: hop.password.clone(),
        smart_card: None,
        server: target.host.clone(),
    };
    let client_name = gtk::glib::host_name().chars().take(15).collect::<String>();
    // The system's authorities vouch for most gateways' certificates. One
    // they don't is asked about, as a server's is, and the gateway tried
    // again trusting that one certificate.
    let mut accepted: Option<String> = None;
    loop {
        let seen = Arc::new(Mutex::new(None::<Vec<u8>>));
        let callback: CertificateValidationCallback = {
            let (seen, accepted) = (seen.clone(), accepted.clone());
            Arc::new(move |der: &[u8], _: &str, why: &str| {
                if accepted.as_deref() == Some(fingerprint(der).as_str()) {
                    return true;
                }
                tracing::info!("the gateway's certificate isn't vouched for: {why}");
                if let Ok(mut seen) = seen.lock() {
                    *seen = Some(der.to_vec());
                }
                false
            })
        };
        let opened = GwClient::connect_with_port_and_certificate_validation(
            &gateway,
            &client_name,
            target.port,
            CertificateValidation::Strict,
            Some(callback),
        )
        .await;
        let error = match opened {
            Ok((client, local)) => {
                tracing::info!("through the RD Gateway {}", hop.host);
                return Ok((Box::new(client), local));
            }
            Err(error) => error,
        };
        let seen = seen.lock().ok().and_then(|mut seen| seen.take());
        if let (None, Some(der)) = (&accepted, seen) {
            let fingerprint = fingerprint(&der);
            let (reply, answer) = oneshot::channel();
            let asked = Event::Certificate {
                host: hop.host.clone(),
                port: hop.port,
                fingerprint: fingerprint.clone(),
                of: Identity::Gateway,
                also: Vec::new(),
                reply,
            };
            if events.send(asked).await.is_err() {
                return Err(Failure::Untrusted);
            }
            match answer.await.unwrap_or(Trust::Rejected) {
                Trust::Known | Trust::Accepted => {
                    accepted = Some(fingerprint);
                    continue;
                }
                Trust::Rejected => return Err(Failure::Untrusted),
            }
        }
        let text = describe(&error);
        tracing::warn!("RD Gateway: {text}");
        let tunnel = match error.kind() {
            GwErrorKind::HttpStatus(401) => Tunnel::Refused,
            GwErrorKind::HttpStatus(403) => Tunnel::Denied,
            // [MS-TSGU] 2.2.6: the HRESULTs, with or without the facility.
            GwErrorKind::GatewayCode(code) => match code & 0xFFFF {
                0x0005 | 0x59DA | 0x59DB | 0x59ED => Tunnel::Denied,
                0x59DD => Tunnel::NoRoute(text),
                0x030C | 0x59F8 | 0x59FA => Tunnel::Refused,
                _ => Tunnel::Other(text),
            },
            GwErrorKind::Connect => match io_error(&error) {
                Some(io) => Tunnel::Unreachable(Unreachable::from_io(io)),
                None => Tunnel::Other(text),
            },
            _ => Tunnel::Other(text),
        };
        return Err(Failure::Gateway(tunnel));
    }
}
