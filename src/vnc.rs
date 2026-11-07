//! The VNC engine: RFB 3.3 to 3.8 (RFC 6143) and the community extensions
//! Linux servers use, run like the RDP engine on a thread of its own and
//! spoken to through [`crate::remote`]. The handshake runs over a stream
//! the engine holds, so it decides when, if ever, the stream becomes TLS.

mod decode;
mod dh;
mod handshake;
mod input;
mod openssl;
mod ra2;
mod reader;
mod security;
mod session;

use tokio::sync::mpsc;

use crate::connection::address_text;
use crate::remote::route::{self, Waking};
use crate::remote::{Command, Ending, Event, Failure, Handle, Target};

/// How long a dropped session keeps trying to come back.
const RECONNECT_FOR: std::time::Duration = std::time::Duration::from_secs(120);

/// Connect to `target`, and keep the session going until it ends.
pub(crate) fn start(target: Target) -> (Handle, mpsc::Receiver<Event>) {
    // VeNCrypt's certificates go through rustls, as RDP's do.
    crate::remote::use_ring();
    let (commands, command_rx) = mpsc::unbounded_channel();
    let (event_tx, events) = mpsc::channel(8);
    // A thread and a runtime per session, as for RDP: one busy session
    // can't slow another.
    let spawned = std::thread::Builder::new()
        .name("vnc-session".into())
        .spawn(move || {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime.block_on(run(target, event_tx, command_rx)),
                Err(e) => {
                    let failure = Failure::Protocol(e.to_string());
                    let _ = event_tx.blocking_send(Event::Ended(Ending::Failed(failure)));
                }
            }
        });
    if let Err(e) = spawned {
        tracing::error!("starting a session thread: {e}");
    }
    (Handle::new(commands), events)
}

async fn run(
    mut target: Target,
    events: mpsc::Sender<Event>,
    mut commands: mpsc::UnboundedReceiver<Command>,
) {
    let ending = match drive(&mut target, &events, &mut commands).await {
        Ok(ending) => ending,
        Err(failure) => Ending::Failed(failure),
    };
    let _ = events.send(Event::Ended(ending)).await;
}

async fn drive(
    target: &mut Target,
    events: &mpsc::Sender<Event>,
    commands: &mut mpsc::UnboundedReceiver<Command>,
) -> Result<Ending, Failure> {
    // Since when the session has been trying to come back, and how long it
    // waits before the next try.
    let mut lost: Option<(std::time::Instant, std::time::Duration)> = None;
    let mut keys = input::Keys::default();
    loop {
        let opened = tokio::select! {
            opened = open(target, events) => opened,
            () = closed(commands) => return Ok(Ending::Closed),
        };
        let opened = match opened {
            Ok(opened) => opened,
            Err(failure) => match &mut lost {
                Some((since, wait)) if failure.passing() && since.elapsed() < RECONNECT_FOR => {
                    tracing::info!("reconnecting failed; trying again in {wait:?}");
                    tokio::select! {
                        () = tokio::time::sleep(*wait) => {}
                        () = closed(commands) => return Ok(Ending::Closed),
                    }
                    *wait = (*wait * 2).min(std::time::Duration::from_secs(10));
                    continue;
                }
                _ => return Err(failure),
            },
        };
        if events
            .send(Event::Protection(opened.protection))
            .await
            .is_err()
        {
            return Ok(Ending::Closed);
        }
        match session::run(opened.stream, opened.init, events, commands, &mut keys).await {
            Err(Failure::Lost(why)) if target.reconnect => {
                tracing::info!("the connection dropped ({why}); reconnecting");
                lost = Some((std::time::Instant::now(), std::time::Duration::from_secs(1)));
                if events.send(Event::Reconnecting).await.is_err() {
                    return Ok(Ending::Closed);
                }
            }
            outcome => return outcome,
        }
    }
}

/// Go through the computer's addresses until one answers as a VNC server
/// and lets this computer in. A computer being woken gets more rounds of
/// them, until it wakes or its time is up.
async fn open(
    target: &mut Target,
    events: &mpsc::Sender<Event>,
) -> Result<handshake::Opened, Failure> {
    let addresses = target.addresses.clone();
    let mut waking = target.wake.map(Waking::new);
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
            let patience = route::patience(waking.is_some(), at + 1 == addresses.len());
            let mut asked = false;
            let mut result = attempt(target, events, patience, &mut asked).await;
            // A server can give up on a connection while its certificate
            // or key waits on the user; once trusted, a second try goes
            // straight through.
            let dropped = matches!(
                result,
                Err(Failure::Rejected | Failure::Lost(_) | Failure::Unreachable(_))
            );
            if asked && dropped {
                tracing::info!("retrying after the question of trust");
                result = attempt(target, events, patience, &mut false).await;
            }
            match result {
                Ok(opened) => return Ok(opened),
                Err(failure) if failure.not_there() => missed.push((address, failure)),
                Err(failure) => return Err(failure),
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

async fn attempt(
    target: &Target,
    events: &mpsc::Sender<Event>,
    patience: std::time::Duration,
    asked: &mut bool,
) -> Result<handshake::Opened, Failure> {
    let (stream, _) = route::open(target, events, patience).await?;
    handshake::run(stream, target, events, asked).await
}

/// Resolves when the interface closes the session; whatever else it sends
/// before there is a session is of no use to one.
async fn closed(commands: &mut mpsc::UnboundedReceiver<Command>) {
    while let Some(command) = commands.recv().await {
        if matches!(command, Command::Close) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::input::{Input, Key};
    use crate::remote::{Frame, Via};

    /// The frame as a PPM picture.
    fn save_ppm(path: &str, frame: &Frame) {
        let mut out = format!("P6\n{} {}\n255\n", frame.width, frame.height).into_bytes();
        for row in frame.pixels.as_ref().chunks(frame.stride) {
            for bgrx in row[..usize::from(frame.width) * 4].chunks(4) {
                out.extend_from_slice(&[bgrx[2], bgrx[1], bgrx[0]]);
            }
        }
        std::fs::write(path, out).expect("writes the picture");
    }

    /// Connects to the VNC server in `MOTE_TEST_VNC` (`host:port`) with
    /// `MOTE_TEST_PASSWORD` (`mote` by default, as `temp/vnc/run.sh` sets
    /// it), and expects `MOTE_TEST_EXPECT`: `closed` (by default) once a
    /// frame came, `refused`, `declined` or `unsupported`.
    /// `MOTE_TEST_PICTURE=<file.ppm>` keeps the last frame;
    /// `MOTE_TEST_TYPE=<text>` types it (lower case letters and spaces)
    /// after the first frame, and keeps the frame two seconds later.
    /// `MOTE_TEST_VNC=localhost:5999 cargo test vnc::tests::live -- --ignored`.
    #[test]
    #[ignore]
    fn live() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_env("MOTE_LOG")
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("mote=info")),
            )
            .with_test_writer()
            .try_init();
        let var = |name: &str, default: &str| std::env::var(name).unwrap_or(default.into());
        let server = std::env::var("MOTE_TEST_VNC").expect("MOTE_TEST_VNC");
        let (host, port) = server.rsplit_once(':').expect("host:port");
        let port: u16 = port.parse().expect("port");
        let target = Target {
            protocol: crate::connection::Protocol::Vnc,
            // MOTE_TEST_UNENCRYPTED=1 allows a server without encryption.
            allow_unencrypted: std::env::var_os("MOTE_TEST_UNENCRYPTED").is_some(),
            host: host.into(),
            port,
            addresses: vec![(host.into(), port)],
            // MOTE_TEST_USER: for the kinds of security that take one.
            username: var("MOTE_TEST_USER", ""),
            password: var("MOTE_TEST_PASSWORD", "mote"),
            domain: String::new(),
            width: 1024,
            height: 768,
            scale: 100,
            quality: Default::default(),
            network: Default::default(),
            clipboard: false,
            clipboard_files: false,
            sound: false,
            microphone: false,
            folder: None,
            via: Via::Direct,
            wake: None,
            reconnect: false,
            keyboard_layout: 0x409,
        };
        let typed = std::env::var("MOTE_TEST_TYPE").ok();
        let (handle, mut events) = start(target);
        let mut frames = 0;
        let mut typing_at = None;
        while let Some(event) = events.blocking_recv() {
            match event {
                Event::Connected { width, height } => eprintln!("connected {width}x{height}"),
                Event::Certificate {
                    fingerprint,
                    of,
                    reply,
                    ..
                } => {
                    eprintln!("identity {of:?} {fingerprint}");
                    let _ = reply.send(crate::remote::Trust::Accepted);
                }
                Event::Protection(protection) => eprintln!("protection {protection:?}"),
                Event::Cursor(_) => eprintln!("cursor"),
                Event::Frame(frame) => {
                    frames += 1;
                    if let Ok(path) = std::env::var("MOTE_TEST_PICTURE") {
                        save_ppm(&path, &frame);
                    }
                    match (&typed, typing_at) {
                        (Some(text), None) => {
                            let input = text.chars().flat_map(|c| {
                                let key = Key {
                                    scancode: scancode(c),
                                    keysym: u32::from(c),
                                };
                                [true, false].map(|down| Input::Key { key, down })
                            });
                            handle.send(Command::Input(input.collect()));
                            typing_at = Some(std::time::Instant::now());
                        }
                        (Some(_), Some(at)) if at.elapsed().as_secs() < 2 => {}
                        _ => handle.send(Command::Close),
                    }
                    handle.send(Command::FrameDone);
                    // An idle desktop sends nothing more: ask again soon.
                    if typing_at.is_some_and(|at| at.elapsed().as_secs() < 2) {
                        let handle = handle.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(std::time::Duration::from_millis(500));
                            handle.send(Command::Input(vec![Input::Pointer { x: 1, y: 1 }]));
                        });
                    }
                }
                Event::Ended(ending) => {
                    let text = match ending {
                        Ending::Closed => "closed".to_owned(),
                        Ending::ServerClosed(_) => "server closed".to_owned(),
                        Ending::Failed(Failure::Refused) => "refused".to_owned(),
                        Ending::Failed(Failure::Declined(why)) => {
                            eprintln!("declined: {why}");
                            "declined".to_owned()
                        }
                        Ending::Failed(Failure::Security(
                            crate::remote::Security::Unsupported(kinds),
                        )) => {
                            eprintln!("unsupported: {kinds}");
                            "unsupported".to_owned()
                        }
                        Ending::Failed(Failure::Security(crate::remote::Security::Unencrypted)) => {
                            "unencrypted".to_owned()
                        }
                        Ending::Failed(Failure::Security(crate::remote::Security::Other(why))) => {
                            format!("security: {why}")
                        }
                        Ending::Failed(Failure::Protocol(e) | Failure::Lost(e)) => {
                            format!("failed: {e}")
                        }
                        Ending::Failed(_) => "failed".to_owned(),
                    };
                    eprintln!("ended: {text}, after {frames} frames");
                    assert_eq!(text, var("MOTE_TEST_EXPECT", "closed"));
                    return;
                }
                _ => {}
            }
        }
    }

    /// The scancode of a lower case letter or a space, on any layout's
    /// keyboard that has it where US English does.
    fn scancode(c: char) -> u16 {
        let rows = ["qwertyuiop", "asdfghjkl", "zxcvbnm"];
        let starts = [0x10, 0x1E, 0x2C];
        for (row, start) in rows.iter().zip(starts) {
            if let Some(at) = row.find(c) {
                return start + at as u16;
            }
        }
        0x39
    }
}
