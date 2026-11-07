//! The connection sequence: the way there, TLS, the certificate check, NLA,
//! and the redirections a server can ask for on the way.

use ironrdp_connector::credssp::CredsspSequence;
use ironrdp_connector::sspi::generator::NetworkRequest;
use ironrdp_connector::{
    BitmapConfig, ClientConnector, ClientConnectorState, Config, ConnectionOutcome,
    ConnectionResult, ConnectorResult, Credentials, DesktopSize, ServerName,
};
use ironrdp_pdu::gcc;
use ironrdp_pdu::nego::SecurityProtocol;
use ironrdp_pdu::rdp::capability_sets::{
    MajorPlatformType, RailSupportLevel, client_codecs_capabilities,
};
use ironrdp_pdu::rdp::client_info::{PerformanceFlags, TimezoneInfo};
use ironrdp_pdu::rdp::server_redirection::{ServerRedirectionFlags, ServerRedirectionPdu};
use ironrdp_tokio::{NetworkClient, TokioFramed};
use tokio::sync::{mpsc, oneshot};
use x509_cert::der::Encode as _;

use super::{Event, Failure, Identity, MAX_HOPS, Target, Trust, Via, describe, rdstls};
use crate::connection::{Network, Quality, address_text};
use crate::remote::fingerprint;

pub(super) use crate::remote::route::Stream;

/// The connection once TLS is up, ready for the session.
pub(super) type Framed = TokioFramed<Box<dyn Stream>>;

/// Connect, following the server's redirections, until a session starts.
/// `redirect` carries a redirection the session itself asked for.
pub(super) async fn connect(
    target: &mut Target,
    mut redirect: Option<Box<ServerRedirectionPdu>>,
    events: &mpsc::Sender<Event>,
    clipboard: &mut super::clipboard::Clipboard,
    audio: &super::audio::Audio,
) -> Result<(ConnectionResult, Framed), Failure> {
    for _ in 0..MAX_HOPS {
        let (outcome, framed) =
            reach(target, redirect.as_deref(), events, clipboard, audio).await?;
        match outcome {
            ConnectionOutcome::Connected(result) => return Ok((result, framed)),
            ConnectionOutcome::Redirected(pdu) => {
                tracing::info!("the server redirected the connection");
                follow(target, &pdu)?;
                redirect = Some(pdu);
            }
        }
    }
    Err(Failure::Protocol("too many redirections".into()))
}

/// Go through the computer's addresses until one answers as a server, which
/// `target` is left pointing at. A computer being woken gets more rounds of
/// them, until it wakes or its time is up.
async fn reach(
    target: &mut Target,
    redirect: Option<&ServerRedirectionPdu>,
    events: &mpsc::Sender<Event>,
    clipboard: &mut super::clipboard::Clipboard,
    audio: &super::audio::Audio,
) -> Result<(ConnectionOutcome, Framed), Failure> {
    // A redirection names the one place to go.
    let addresses = match redirect {
        Some(_) => vec![(target.host.clone(), target.port)],
        None => target.addresses.clone(),
    };
    let mut waking = target.wake.map(crate::remote::route::Waking::new);
    loop {
        if let Some(waking) = &mut waking {
            waking.send().await;
        }
        let mut missed = Vec::new();
        for (at, (host, port)) in addresses.iter().enumerate() {
            (target.host, target.port) = (host.clone(), *port);
            let address = address_text(host, *port);
            if addresses.len() > 1 {
                tracing::info!("trying {address}");
                let _ = events.send(Event::Trying(address.clone())).await;
            }
            let patience =
                crate::remote::route::patience(waking.is_some(), at + 1 == addresses.len());
            let mut result = attempt(target, redirect, patience, events, clipboard, audio).await;
            // A server can give up on a connection while its certificate
            // waits on the user; once trusted, a second try goes straight
            // through.
            if matches!(result, Err(Attempt::AfterQuestion(_))) {
                tracing::info!("retrying after the certificate question");
                result = attempt(target, redirect, patience, events, clipboard, audio).await;
            }
            match result {
                Ok(connected) => return Ok(connected),
                Err(Attempt::Failed(failure) | Attempt::AfterQuestion(failure)) => {
                    if !failure.not_there() {
                        return Err(failure);
                    }
                    missed.push((address, failure));
                }
            }
        }
        let failure = Failure::from_missed(missed);
        let again = match &mut waking {
            Some(waking) if failure.asleep() => waking.wait(&target.host, events).await,
            _ => false,
        };
        if !again {
            return Err(failure);
        }
    }
}

/// Point `target` where a Server Redirection PDU says to go.
pub(super) fn follow(target: &mut Target, pdu: &ServerRedirectionPdu) -> Result<(), Failure> {
    let flags = pdu.flags;
    if !flags.contains(ServerRedirectionFlags::NO_REDIRECT) {
        let host = pdu
            .target_fqdn
            .as_deref()
            .filter(|_| flags.contains(ServerRedirectionFlags::TARGET_FQDN))
            .or(pdu
                .target_net_address
                .as_deref()
                .filter(|_| flags.contains(ServerRedirectionFlags::TARGET_NET_ADDRESS)));
        if let Some(host) = host
            .map(|h| h.trim_end_matches('\0'))
            .filter(|h| !h.is_empty())
        {
            target.host = host.to_owned();
        }
    }
    if let Some(username) = &pdu.username {
        target.username = username.trim_end_matches('\0').to_owned();
    }
    if let Some(domain) = &pdu.domain {
        target.domain = domain.trim_end_matches('\0').to_owned();
    }
    // An encrypted password is for RDSTLS to pass on as it is; a plain one,
    // in UTF-16, signs in to the target.
    if let Some(password) = pdu.password.as_deref().filter(|_| !rdstls::needed(pdu)) {
        target.password = utf16(password);
    }
    Ok(())
}

fn utf16(bytes: &[u8]) -> String {
    let (units, _) = bytes.as_chunks::<2>();
    let units: Vec<u16> = units.iter().map(|u| u16::from_le_bytes(*u)).collect();
    String::from_utf16_lossy(&units)
        .trim_end_matches('\0')
        .to_owned()
}

enum Attempt {
    Failed(Failure),
    /// Failed after asking the user about the certificate.
    AfterQuestion(Failure),
}

impl From<Failure> for Attempt {
    fn from(failure: Failure) -> Self {
        Self::Failed(failure)
    }
}

async fn attempt(
    target: &Target,
    redirect: Option<&ServerRedirectionPdu>,
    patience: std::time::Duration,
    events: &mpsc::Sender<Event>,
    clipboard: &mut super::clipboard::Clipboard,
    audio: &super::audio::Audio,
) -> Result<(ConnectionOutcome, Framed), Attempt> {
    let (stream, client_addr) = crate::remote::route::open(target, events, patience).await?;
    let mut framed = TokioFramed::new(stream);

    let mut connector = ClientConnector::new(config(target, audio), client_addr)
        .with_static_channel(channels(target.quality, audio, events));
    if let Some(channel) = clipboard.channel() {
        connector = connector.with_static_channel(channel);
    }
    if let Some(channel) = audio.static_channel() {
        connector = connector.with_static_channel(channel);
    }
    if let Some(folder) = &target.folder {
        let computer = gtk::glib::host_name().chars().take(15).collect::<String>();
        connector = connector.with_static_channel(super::devices::channel(folder, &computer));
    }
    if let Some(pdu) = redirect {
        connector = with_redirection(connector, pdu);
    }

    // A redirection with an encrypted password takes RDSTLS, which the
    // connector can't ask for: ask here, and let it take the reply as if it
    // had asked itself.
    let rdstls = redirect.filter(|pdu| rdstls::needed(pdu));
    let should_upgrade = match rdstls {
        Some(pdu) => {
            rdstls::send_request(&mut framed, pdu).await?;
            connector.state = ClientConnectorState::ConnectionInitiationWaitConfirm {
                requested_protocol: SecurityProtocol::RDSTLS,
            };
            ironrdp_tokio::connect_begin(&mut framed, &mut connector)
                .await
                .map_err(|e| Failure::opening(&e))?
        }
        None => ironrdp_tokio::connect_begin(&mut framed, &mut connector)
            .await
            .map_err(|e| Failure::opening(&e))?,
    };
    let (stream, leftover) = framed.into_inner();
    // The certificate is checked here, by fingerprint, before anything is
    // sent: servers on a LAN have self-signed certificates no authority
    // could vouch for.
    let (tls, certificate) = ironrdp_tls::upgrade_with_certificate_validation(
        stream,
        &target.host,
        ironrdp_tls::CertificateValidation::DangerouslyAcceptInvalidCertificate,
    )
    .await
    .map_err(|e| Failure::tls(&e))?;
    let mut tls = super::tap::Tap::new(tls);
    let der = certificate
        .to_der()
        .map_err(|e| Failure::Protocol(e.to_string()))?;
    let vouched = redirect
        .and_then(|pdu| pdu.target_certificate.as_ref())
        .and_then(|c| c.certificate_der())
        .is_some_and(|expected| expected == der.as_slice());
    if let Some(pdu) = redirect {
        tracing::info!(
            "following a redirection: flags {:?}, RDSTLS {}, certificate as announced {}",
            pdu.flags,
            rdstls.is_some(),
            vouched
        );
    }
    let mut asked = false;
    if !vouched {
        asked = ask_trust(target, redirect.is_some(), &der, events).await?;
    }
    if let Some(server) = super::server::Server::from_certificate(&der) {
        tracing::debug!("by its certificate, the server is {server:?}");
        let _ = events.send(Event::Server(server)).await;
    }
    let fail = |failure: Failure| match failure {
        Failure::Refused | Failure::Untrusted => Attempt::Failed(failure),
        _ if asked => Attempt::AfterQuestion(failure),
        _ => Attempt::Failed(failure),
    };

    if let Some(pdu) = rdstls {
        rdstls::authenticate(&mut tls, &target.username, &target.domain, pdu)
            .await
            .map_err(fail)?;
    }
    let upgraded = ironrdp_tokio::mark_as_upgraded(should_upgrade, &mut connector);
    let tls: Box<dyn Stream> = Box::new(tls);
    let mut framed = TokioFramed::new_with_leftover(tls, leftover);
    let public_key = ironrdp_tls::extract_tls_server_public_key(&certificate)
        .ok_or_else(|| Failure::Protocol("the certificate has no public key".into()))?
        .to_owned();
    let server_name = ServerName::new(target.host.clone());

    if connector.should_perform_credssp() {
        credssp(
            &mut connector,
            &mut framed,
            server_name.clone(),
            public_key.clone(),
        )
        .await
        .map_err(|e| {
            tracing::warn!("NLA: {}", describe(&e));
            fail(Failure::Refused)
        })?;
    }
    let outcome = ironrdp_tokio::connect_finalize(
        upgraded,
        connector,
        &mut framed,
        &mut NoKdc,
        server_name,
        public_key,
        None,
    )
    .await
    .map_err(|e| fail(Failure::from_connector(&e)))?;
    Ok((outcome, framed))
}

/// Ask the interface whether to trust the certificate; `Ok(true)` when the
/// user had to answer, rather than the certificate being known.
async fn ask_trust(
    target: &Target,
    redirected: bool,
    der: &[u8],
    events: &mpsc::Sender<Event>,
) -> Result<bool, Attempt> {
    let (reply, answer) = oneshot::channel();
    // Behind a tunnel or a gateway, the name is the one it goes by there,
    // where "localhost" is another machine than here.
    let named = |host: &str| match &target.via {
        Via::Direct => host.to_owned(),
        Via::Ssh(hop) | Via::Gateway(hop) => format!("{host} via {}", hop.host),
    };
    // Where a redirection leads is a computer of its own.
    let also = if redirected {
        Vec::new()
    } else {
        target
            .addresses
            .iter()
            .map(|(host, port)| (named(host), *port))
            .collect()
    };
    events
        .send(Event::Certificate {
            host: named(&target.host),
            port: target.port,
            fingerprint: fingerprint(der),
            of: Identity::Server,
            also,
            reply,
        })
        .await
        .map_err(|_| Failure::Untrusted)?;
    match answer.await.unwrap_or(Trust::Rejected) {
        Trust::Known => Ok(false),
        Trust::Accepted => Ok(true),
        Trust::Rejected => Err(Failure::Untrusted.into()),
    }
}

/// NLA, run here rather than inside `connect_finalize` so that a failure
/// in it can be told apart: it means the credentials were refused.
async fn credssp(
    connector: &mut ClientConnector,
    framed: &mut Framed,
    server_name: ServerName,
    public_key: Vec<u8>,
) -> ConnectorResult<()> {
    let ClientConnectorState::Credssp { selected_protocol } = connector.state else {
        return Err(ironrdp_connector::general_err!("not at NLA"));
    };
    let domain = connector.config.domain.clone();
    let (sequence, request) = CredsspSequence::init(
        connector.config.credentials.clone(),
        domain.as_deref(),
        selected_protocol,
        server_name,
        public_key,
        None,
    )?;
    let mut buf = ironrdp_core::WriteBuf::new();
    ironrdp_tokio::perform_credssp(framed, &mut NoKdc, &mut buf, sequence, request).await?;
    connector.mark_credssp_as_done();
    Ok(())
}

/// How long into its picture a server offered H.264 has to send some
/// before it counts as not sending it: what comes first may be in
/// another codec.
const NO_H264_AFTER: std::time::Duration = std::time::Duration::from_secs(3);

/// The dynamic channels: the Graphics Pipeline, with H.264 where the system
/// has OpenH264, whose output lands in the session's image by way of the
/// session itself, and Display Control. Each
/// is made afresh whenever the server opens it: xrdp opens both again after
/// a resize.
fn channels(
    quality: Quality,
    audio: &super::audio::Audio,
    events: &mpsc::Sender<Event>,
) -> ironrdp_dvc::DrdynvcClient {
    use ironrdp_egfx::pdu::Codec1Type;
    /// What the server chose, logged for measurements; and, when it was
    /// offered H.264, whether it sends it.
    struct Pictures {
        codec: Option<Codec1Type>,
        /// Until the server's first H.264, or until it has had long enough
        /// to send some: when its picture began.
        watch: Option<(Option<std::time::Instant>, mpsc::Sender<Event>)>,
    }
    impl ironrdp_egfx::client::GraphicsPipelineHandler for Pictures {
        fn on_capabilities_confirmed(&mut self, caps: &ironrdp_egfx::pdu::CapabilitySet) {
            tracing::info!(target: "mote::stats", "graphics pipeline {caps:?}");
        }

        fn on_bitmap_updated(&mut self, update: &ironrdp_egfx::client::BitmapUpdate) {
            if self.codec != Some(update.codec_id) {
                self.codec = Some(update.codec_id);
                tracing::info!(target: "mote::stats", "codec {:?}", update.codec_id);
            }
            let avc = matches!(
                update.codec_id,
                Codec1Type::Avc420 | Codec1Type::Avc444 | Codec1Type::Avc444v2
            );
            let Some((began, events)) = &mut self.watch else {
                return;
            };
            if avc {
                self.watch = None;
            } else if began.get_or_insert_with(std::time::Instant::now).elapsed() >= NO_H264_AFTER {
                // Called on the session's task, which mustn't wait here.
                let _ = events.try_send(Event::NoH264);
                self.watch = None;
            }
        }
    }
    let events = events.clone();
    let mut channels = ironrdp_dvc::DrdynvcClient::new()
        .with_dynamic_channel_factory(ironrdp_egfx::CHANNEL_NAME, move || {
            let h264 = match quality {
                Quality::RemoteFx => None,
                _ => super::h264::decoder().map(|(_, d)| {
                    let avc444 = quality == Quality::Best;
                    Box::new(super::h264::Decoder::new(d, avc444)) as Box<_>
                }),
            };
            let _ = events.try_send(Event::Graphics {
                h264: h264.is_some(),
            });
            let pictures = Pictures {
                codec: None,
                watch: h264.is_some().then(|| (None, events.clone())),
            };
            ironrdp_egfx::client::GraphicsPipelineClient::new(Box::new(pictures), h264)
        })
        .with_dynamic_channel_factory(ironrdp_displaycontrol::CHANNEL_NAME, || {
            ironrdp_displaycontrol::client::DisplayControlClient::new(|_| Ok(Vec::new()))
        });
    if let Some(playback) = audio.playback() {
        channels = channels
            .with_dynamic_channel_factory(ironrdp_rdpsnd::client::DVC_CHANNEL_NAME, playback);
    }
    if let Some(capture) = audio.capture() {
        channels = channels.with_dynamic_channel_factory(ironrdp_rdpeai::CHANNEL_NAME, capture);
    }
    channels
}

fn with_redirection(connector: ClientConnector, pdu: &ServerRedirectionPdu) -> ClientConnector {
    let mut connector = connector.with_cluster_data(gcc::ClientClusterData {
        flags: gcc::RedirectionFlags::REDIRECTION_SUPPORTED
            | gcc::RedirectionFlags::REDIRECTED_SESSION_FIELD_VALID,
        redirection_version: gcc::RedirectionVersion::V6,
        redirected_session_id: pdu.session_id,
    });
    if let Some(token) = rdstls::routing_token(pdu) {
        connector = connector.with_load_balance_info(token);
    }
    if let Some(password) = &pdu.password {
        connector = connector.with_redirection_password_cookie(password.clone());
    }
    connector
}

fn config(target: &Target, audio: &super::audio::Audio) -> Config {
    let client_name = gtk::glib::host_name().chars().take(15).collect::<String>();
    Config {
        desktop_size: DesktopSize {
            width: target.width,
            height: target.height,
        },
        monitor_layout: None,
        desktop_scale_factor: target.scale,
        desktop_physical_size: Some(super::physical_size(
            u32::from(target.width),
            u32::from(target.height),
            target.scale,
        )),
        desktop_orientation: None,
        // Of the three the protocol allows, the nearest.
        device_scale_factor: Some(match target.scale {
            ..125 => 100,
            125..165 => 140,
            _ => 180,
        }),
        monitor_extended_layout: None,
        relative_mouse_input: false,
        enable_tls: true,
        enable_credssp: true,
        enable_standard_rdp_security: false,
        credentials: Credentials::UsernamePassword {
            username: target.username.clone(),
            password: target.password.clone(),
        },
        domain: Some(target.domain.clone()).filter(|d| !d.is_empty()),
        client_build: 0,
        client_name,
        keyboard_type: gcc::KeyboardType::IBM_ENHANCED,
        keyboard_subtype: 0,
        keyboard_functional_keys_count: 12,
        // Keys go as scancodes whatever this says; xrdp sets the session's
        // layout by it.
        keyboard_layout: target.keyboard_layout,
        connection_type: match target.network {
            Network::Lan => gcc::ConnectionType::Lan,
            Network::Wan => gcc::ConnectionType::Wan,
            Network::BroadbandHigh => gcc::ConnectionType::BroadbandHigh,
            Network::Satellite => gcc::ConnectionType::Satellite,
            Network::BroadbandLow => gcc::ConnectionType::BroadbandLow,
            Network::Modem => gcc::ConnectionType::Modem,
        },
        ime_file_name: String::new(),
        bitmap: Some(BitmapConfig {
            lossy_compression: true,
            color_depth: 32,
            codecs: client_codecs_capabilities(&[]).unwrap_or_default(),
        }),
        dig_product_id: String::new(),
        client_dir: String::new(),
        alternate_shell: String::new(),
        work_dir: String::new(),
        remote_application_mode: false,
        rail_support_level: RailSupportLevel::empty(),
        platform: MajorPlatformType::UNIX,
        hardware_id: None,
        request_data: None,
        // xrdp signs in with the account in the Client Info PDU only when
        // asked to; servers that did NLA already ignore it.
        autologon: !target.password.is_empty(),
        enable_audio_playback: audio.plays(),
        enable_audio_capture: audio.records(),
        performance_flags: PerformanceFlags::default(),
        license_cache: None,
        timezone_info: TimezoneInfo::default(),
        compression_type: None,
        enable_server_pointer: true,
        pointer_software_rendering: false,
        multitransport_flags: None,
        support_dyn_vc_gfx_protocol: true,
    }
}

/// Kerberos would need a KDC; Mote signs in with NTLM, which needs none.
struct NoKdc;

impl NetworkClient for NoKdc {
    async fn send(&mut self, _: &NetworkRequest) -> ConnectorResult<Vec<u8>> {
        Err(ironrdp_connector::general_err!("Kerberos is not supported"))
    }
}
