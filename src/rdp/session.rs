//! A running session: frames in, input out, until one side ends it.

use ironrdp_connector::ConnectionResult;
use ironrdp_connector::Sequence as _;
use ironrdp_connector::connection_activation::ConnectionActivationState;
use ironrdp_displaycontrol::pdu::MonitorLayoutEntry;
use ironrdp_graphics::image_processing::PixelFormat;
use ironrdp_pdu::geometry::{InclusiveRectangle, Rectangle as _};
use ironrdp_pdu::rdp::server_redirection::ServerRedirectionPdu;
use ironrdp_session::image::DecodedImage;
use ironrdp_session::{
    ActiveStage, ActiveStageBuilder, ActiveStageOutput, GracefulDisconnectReason,
};
use ironrdp_tokio::{FramedWrite as _, split_tokio_framed};
use tokio::sync::mpsc;

use super::connect::Framed;
use super::{Command, Cursor, Disconnect, Ending, Event, Failure, Frame, Rect, describe};

pub(super) enum Outcome {
    Ended(Ending),
    /// The server handed the session on; connect again where it says.
    Redirect(Box<ServerRedirectionPdu>),
}

/// What the loop holds between frames.
struct Desktop {
    image: DecodedImage,
    /// Changed since the last frame went out.
    damage: Vec<Rect>,
    /// When the oldest of that change was decoded.
    since: Option<std::time::Instant>,
    /// A frame is with the interface, not yet drawn.
    in_flight: bool,
    /// A frame has gone out.
    shown: bool,
    frames: crate::remote::frames::Frames,
}

impl Desktop {
    fn new(width: u16, height: u16) -> Self {
        Self {
            // BGRX is what GDK takes without converting.
            image: DecodedImage::new(PixelFormat::BgrX32, width, height),
            damage: Vec::new(),
            since: None,
            in_flight: false,
            shown: false,
            frames: crate::remote::frames::Frames::new(),
        }
    }
}

/// What a connection keeps from one session to the next, when it
/// reconnects or is redirected.
pub(super) struct Kept<'a> {
    pub(super) input: &'a mut ironrdp_input::Database,
    pub(super) clipboard: &'a mut super::clipboard::Clipboard,
    pub(super) audio: &'a mut super::audio::Audio,
}

pub(super) async fn run(
    framed: Framed,
    result: ConnectionResult,
    events: &mpsc::Sender<Event>,
    commands: &mut mpsc::UnboundedReceiver<Command>,
    asked: &mut (u32, u32, u32),
    kept: Kept<'_>,
) -> Result<Outcome, Failure> {
    let Kept {
        input,
        clipboard,
        audio,
    } = kept;
    let (mut reader, mut writer) = split_tokio_framed(framed);
    let size = result.desktop_size;
    let mut desktop = Desktop::new(size.width, size.height);
    send(
        events,
        Event::Connected {
            width: size.width,
            height: size.height,
        },
    )
    .await?;
    let activation = result.activation_factory;
    let mut stage = ActiveStageBuilder {
        static_channels: result.static_channels,
        user_channel_id: result.user_channel_id,
        io_channel_id: result.io_channel_id,
        message_channel_id: result.message_channel_id,
        share_id: result.share_id,
        compression_type: result.compression_type,
        enable_server_pointer: result.enable_server_pointer,
        pointer_software_rendering: result.pointer_software_rendering,
    }
    .build();

    // The reason the server gave, ahead of ending the session.
    let mut reason = None;
    // A resize asked for before Display Control is up, sent once it is.
    let mut resize = None;
    // A monitor layout asked for before Display Control is up.
    let mut layout: Option<Vec<super::Monitor>> = None;
    let mut stats = crate::stats::Stats::new("engine");
    let mut received = 0;
    // Lets go of clipboard locks and transfers the server stopped answering.
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
    // What came for the channels while the server reactivated, taken up
    // before anything new is read.
    let mut held: std::collections::VecDeque<Vec<u8>> = std::collections::VecDeque::new();
    loop {
        let outputs = tokio::select! {
            frame = async {
                match held.pop_front() {
                    Some(frame) => Ok((ironrdp_pdu::Action::X224, ironrdp_tokio::bytes::BytesMut::from(&frame[..]))),
                    None => reader.read_pdu().await,
                }
            } => {
                let (action, mut payload) = match frame {
                    Ok(frame) => frame,
                    // A server that said why can close without more ado.
                    Err(_) if reason.is_some() => {
                        return Ok(Outcome::Ended(Ending::ServerClosed(reason)));
                    }
                    Err(e) => return Err(Failure::Lost(describe(&e))),
                };
                if action == ironrdp_pdu::Action::X224
                    && let Some(info) = super::compat::error_info(&payload)
                {
                    reason = Disconnect::from_error_info(info).or(reason);
                }
                if action == ironrdp_pdu::Action::X224 && super::compat::normalize_redirection(&mut payload) {
                    tracing::debug!("took a FreeRDP-style redirection header");
                }
                received += payload.len() as u64;
                stats.count("kB in", received / 1024);
                received %= 1024;
                let start = std::time::Instant::now();
                let outputs = stage
                    .process(&mut desktop.image, action, &payload, reader.last_read_at())
                    .map_err(|e| failed(&e, desktop.shown))?;
                // Timed only when it drew something: most messages are small.
                if outputs.iter().any(|o| matches!(o, ActiveStageOutput::GraphicsUpdate(_))) {
                    stats.time("decode", start);
                }
                outputs
            }
            command = commands.recv() => match command {
                Some(Command::Input(sent)) => stage
                    .process_fastpath_input(&mut desktop.image, &super::input::events(input, sent))
                    .map_err(|e| Failure::Protocol(describe(&e)))?,
                Some(Command::FrameDone) => {
                    desktop.in_flight = false;
                    Vec::new()
                }
                Some(Command::Resize { width, height, scale }) => {
                    resize = Some((width, height, scale));
                    layout = None;
                    Vec::new()
                }
                Some(Command::Layout(monitors)) => {
                    layout = Some(monitors);
                    resize = None;
                    Vec::new()
                }
                Some(Command::Clipboard(command)) => clipboard.command(command, &mut stage)?,
                Some(Command::Close) | None => {
                    // Ask the server to end the session, without waiting
                    // for it to agree: the window is already gone.
                    if let Ok(outputs) = stage.graceful_shutdown() {
                        for output in outputs {
                            if let ActiveStageOutput::ResponseFrame(frame) = output {
                                let _ = writer.write_all(&frame).await;
                            }
                        }
                    }
                    return Ok(Outcome::Ended(Ending::Closed));
                }
            },
            Some(note) = clipboard.notes.recv() => clipboard.note(note, &mut stage, events).await?,
            (format, content) = super::clipboard::answered(&mut clipboard.answers) => {
                clipboard.answer(format, content, &mut stage)?
            }
            _ = tick.tick(), if clipboard.on() => clipboard.tick(&mut stage)?,
            Some(messages) = audio.packets.recv() => {
                let frame = stage
                    .encode_dvc_messages(messages)
                    .map_err(|e| Failure::Protocol(describe(&e)))?;
                vec![ActiveStageOutput::ResponseFrame(frame)]
            }
        };

        for output in outputs {
            match output {
                ActiveStageOutput::ResponseFrame(frame) => {
                    if !frame.is_empty() {
                        writer
                            .write_all(&frame)
                            .await
                            .map_err(|e| Failure::Lost(describe(&e)))?;
                    }
                }
                ActiveStageOutput::GraphicsUpdate(region) => {
                    desktop.damage.push(rect(&region));
                    desktop.since.get_or_insert_with(std::time::Instant::now);
                }
                ActiveStageOutput::PointerDefault => {
                    send(events, Event::Cursor(Cursor::Default)).await?;
                }
                ActiveStageOutput::PointerHidden => {
                    send(events, Event::Cursor(Cursor::Hidden)).await?;
                }
                ActiveStageOutput::PointerBitmap(pointer) => {
                    let pointer = crate::remote::Pointer {
                        width: pointer.width,
                        height: pointer.height,
                        hotspot_x: pointer.hotspot_x,
                        hotspot_y: pointer.hotspot_y,
                        rgba: pointer.bitmap_data.clone(),
                    };
                    send(events, Event::Cursor(Cursor::Bitmap(pointer.into()))).await?;
                }
                ActiveStageOutput::DeactivateAll => {
                    if let Some(reason) = reactivate(
                        &mut stage,
                        &activation,
                        &mut reader,
                        &mut writer,
                        &mut desktop,
                        &mut held,
                    )
                    .await?
                    {
                        return Ok(Outcome::Ended(Ending::ServerClosed(Some(reason))));
                    }
                    send(
                        events,
                        Event::Connected {
                            width: desktop.image.width(),
                            height: desktop.image.height(),
                        },
                    )
                    .await?;
                }
                ActiveStageOutput::Terminate(why) => {
                    let said = match why {
                        GracefulDisconnectReason::Other(text) => Some(Disconnect::Other(text)),
                        _ => None,
                    };
                    return Ok(Outcome::Ended(Ending::ServerClosed(reason.or(said))));
                }
                ActiveStageOutput::ServerRedirection(pdu) => return Ok(Outcome::Redirect(pdu)),
                _ => {}
            }
        }

        if let Some(monitors) = layout.take() {
            layout = send_layout(&mut stage, &mut writer, events, asked, monitors).await?;
        }
        if let Some(size) = resize.take() {
            resize = send_resize(&mut stage, &mut writer, events, asked, size).await?;
        }

        if !desktop.damage.is_empty() && !desktop.in_flight {
            let image = &desktop.image;
            let start = std::time::Instant::now();
            let damage = std::mem::take(&mut desktop.damage);
            let frame = Frame {
                width: image.width(),
                height: image.height(),
                stride: image.stride(),
                pixels: desktop.frames.make(image.data(), image.stride(), &damage),
                damage,
                made: desktop.since.take().unwrap_or(start),
            };
            stats.time("copy", start);
            stats.count("frames", 1);
            desktop.in_flight = true;
            desktop.shown = true;
            send(events, Event::Frame(frame)).await?;
        }
    }
}

/// The Deactivation-Reactivation Sequence: the server renegotiates, often
/// with a new desktop size. `Ok(Some(reason))` when the server ends the
/// session instead, as KRdp does when the client can't decode H.264.
async fn reactivate(
    stage: &mut ActiveStage,
    activation: &ironrdp_connector::connection_activation::ConnectionActivationFactory,
    reader: &mut ironrdp_tokio::TokioFramed<tokio::io::ReadHalf<Box<dyn super::connect::Stream>>>,
    writer: &mut ironrdp_tokio::TokioFramed<tokio::io::WriteHalf<Box<dyn super::connect::Stream>>>,
    desktop: &mut Desktop,
    held: &mut std::collections::VecDeque<Vec<u8>>,
) -> Result<Option<Disconnect>, Failure> {
    let mut sequence = activation.create();
    let mut buf = ironrdp_core::WriteBuf::new();
    loop {
        buf.clear();
        // Read here rather than in IronRDP's step, which turns the server's
        // Set Error Info into English.
        let written = match sequence.next_pdu_hint() {
            Some(hint) => {
                let pdu = reader
                    .read_by_hint(hint)
                    .await
                    .map_err(|e| Failure::Lost(describe(&e)))?;
                if let Some(reason) =
                    super::compat::error_info(&pdu).and_then(Disconnect::from_error_info)
                {
                    return Ok(Some(reason));
                }
                // xrdp goes on sending on its static channels meanwhile,
                // which IronRDP's sequence takes for its own and fails on.
                if super::compat::channel_of(&pdu)
                    .is_some_and(|channel| channel != sequence.io_channel_id())
                {
                    held.push_back(pdu.to_vec());
                    continue;
                }
                sequence.step(&pdu, reader.last_read_at(), &mut buf)
            }
            None => sequence.step_no_input(&mut buf),
        }
        .map_err(|e| failed(&e, desktop.shown))?;
        if written.size().is_some() {
            writer
                .write_all(buf.filled())
                .await
                .map_err(|e| Failure::Lost(describe(&e)))?;
        }
        if let ConnectionActivationState::Finalized {
            desktop_size,
            share_id,
            static_channel_chunk_size,
            enable_server_pointer,
            pointer_software_rendering,
            ..
        } = sequence.connection_activation_state()
        {
            if !stage.reactivate(
                sequence.io_channel_id(),
                sequence.user_channel_id(),
                share_id,
                enable_server_pointer,
                pointer_software_rendering,
                static_channel_chunk_size,
            ) {
                return Err(Failure::Protocol(
                    "the server's channel settings are invalid".into(),
                ));
            }
            *desktop = Desktop::new(desktop_size.width, desktop_size.height);
            return Ok(None);
        }
    }
}

/// Ask the server for a new desktop size over Display Control. Returns the
/// request back when the channel isn't up yet; a server without it keeps
/// its size, and the picture is scaled instead.
async fn send_resize(
    stage: &mut ActiveStage,
    writer: &mut ironrdp_tokio::TokioFramed<tokio::io::WriteHalf<Box<dyn super::connect::Stream>>>,
    events: &mpsc::Sender<Event>,
    asked: &mut (u32, u32, u32),
    (width, height, scale): (u32, u32, u32),
) -> Result<Option<(u32, u32, u32)>, Failure> {
    let (width, height) = MonitorLayoutEntry::adjust_display_size(width, height);
    if *asked == (width, height, scale) {
        return Ok(None);
    }
    match stage.display_control_ready() {
        Some(true) => {}
        Some(false) => return Ok(Some((width, height, scale))),
        None => {
            tracing::debug!("the server has no Display Control; scaling instead");
            send(events, Event::KeepsSize).await?;
            return Ok(None);
        }
    }
    match stage.encode_resize(
        width,
        height,
        Some(scale),
        Some(super::physical_size(width, height, scale)),
    ) {
        Some(Ok(frame)) => {
            tracing::debug!("asking for {width}×{height} at {scale}%");
            *asked = (width, height, scale);
            writer
                .write_all(&frame)
                .await
                .map_err(|e| Failure::Lost(describe(&e)))?;
        }
        Some(Err(e)) => tracing::warn!("resizing: {}", describe(&e)),
        None => return Ok(Some((width, height, scale))),
    }
    Ok(None)
}

/// Ask the server to spread the desktop over several monitors. Returns the
/// layout back when the channel isn't up yet; a server without the channel,
/// or with room for fewer monitors, is reported.
async fn send_layout(
    stage: &mut ActiveStage,
    writer: &mut ironrdp_tokio::TokioFramed<tokio::io::WriteHalf<Box<dyn super::connect::Stream>>>,
    events: &mpsc::Sender<Event>,
    asked: &mut (u32, u32, u32),
    monitors: Vec<super::Monitor>,
) -> Result<Option<Vec<super::Monitor>>, Failure> {
    use ironrdp_displaycontrol::pdu::{
        DeviceScaleFactor, DisplayControlMonitorLayout, MonitorLayoutEntry,
    };
    match stage.display_control_ready() {
        Some(true) => {}
        Some(false) => return Ok(Some(monitors)),
        None => {
            send(events, Event::OneMonitor).await?;
            return Ok(None);
        }
    }
    let room = stage
        .display_control_capabilities()
        .map_or(1, |c| c.max_num_monitors());
    if (room as usize) < monitors.len() {
        tracing::info!("the server takes {room} monitors, not {}", monitors.len());
        send(events, Event::OneMonitor).await?;
        return Ok(None);
    }
    let entries: Result<Vec<_>, _> = monitors
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let (width, height) = MonitorLayoutEntry::adjust_display_size(m.width, m.height);
            let entry = if i == 0 {
                MonitorLayoutEntry::new_primary(width, height)
            } else {
                MonitorLayoutEntry::new_secondary(width, height)
            }?;
            let scale = MonitorLayoutEntry::adjust_desktop_scale_factor(m.scale);
            let (physical_width, physical_height) = super::physical_size(width, height, m.scale);
            entry
                .with_position(m.x, m.y)?
                .with_desktop_scale_factor(scale)?
                .with_device_scale_factor(DeviceScaleFactor::from_desktop_scale_factor(scale))
                .with_physical_dimensions(physical_width, physical_height)
        })
        .collect();
    let encoded = entries
        .and_then(|entries| DisplayControlMonitorLayout::new(&entries))
        .map_err(|e| Failure::Protocol(e.to_string()))
        .map(|layout| stage.encode_monitor_layout(&layout));
    match encoded? {
        Some(Ok(frame)) => {
            tracing::debug!("asking for {monitors:?}");
            // Whatever size comes next is new.
            *asked = (0, 0, 0);
            writer
                .write_all(&frame)
                .await
                .map_err(|e| Failure::Lost(describe(&e)))?;
        }
        Some(Err(e)) => tracing::warn!("spreading over monitors: {}", describe(&e)),
        None => return Ok(Some(monitors)),
    }
    Ok(None)
}

/// A session error. A hang-up before the first frame, with no reason given,
/// is KRdp's answer to a wrong password.
fn failed(error: &(dyn std::error::Error + 'static), shown: bool) -> Failure {
    let text = describe(error);
    if !shown && text.contains("disconnect provider ultimatum") {
        Failure::Rejected
    } else {
        Failure::Protocol(text)
    }
}

fn rect(region: &InclusiveRectangle) -> Rect {
    Rect {
        x: i32::from(region.left),
        y: i32::from(region.top),
        width: i32::from(region.width()),
        height: i32::from(region.height()),
    }
}

async fn send(events: &mpsc::Sender<Event>, event: Event) -> Result<(), Failure> {
    // The interface has gone: nobody is left to show the session to.
    events
        .send(event)
        .await
        .map_err(|_| Failure::Lost("the window closed".into()))
}
