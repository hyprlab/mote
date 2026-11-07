//! The commands a connection runs before connecting and after
//! disconnecting: through `sh -c`, with the connection in the environment
//! rather than pasted into the command, where a name could break it.

use gtk::{gio, glib};
use relm4::prelude::*;

use super::{Page, Session, SessionInput};
use crate::connection::Connection;
use crate::i18n::i18n;
use crate::remote;

fn spawn(
    command: &str,
    connection: &Connection,
    flags: gio::SubprocessFlags,
) -> Result<gio::Subprocess, String> {
    let launcher = gio::SubprocessLauncher::new(flags);
    for (name, value) in [
        ("MOTE_NAME", connection.title()),
        ("MOTE_GROUP", connection.group.as_str()),
        ("MOTE_HOST", connection.host.as_str()),
        ("MOTE_PORT", &connection.port.to_string()),
        ("MOTE_USER", connection.username.as_str()),
    ] {
        launcher.setenv(name, value, true);
    }
    launcher
        .spawn(&["sh".as_ref(), "-c".as_ref(), command.as_ref()])
        .map_err(|e| e.message().to_owned())
}

/// Run the command before connecting, and wait for it. `Err` holds what
/// went wrong: the last line it wrote to stderr, or its exit status.
pub(super) async fn before(command: String, connection: Connection) -> Result<(), String> {
    let process = spawn(&command, &connection, gio::SubprocessFlags::STDERR_PIPE)?;
    let (_, stderr) = process
        .communicate_utf8_future(None)
        .await
        .map_err(|e| e.message().to_owned())?;
    if process.is_successful() {
        return Ok(());
    }
    let said = stderr
        .as_deref()
        .and_then(|text| text.lines().rev().find(|l| !l.trim().is_empty()))
        .map(|line| line.trim().to_owned());
    Err(said.unwrap_or_else(|| {
        if process.has_exited() {
            format!("exit status {}", process.exit_status())
        } else {
            format!("signal {}", process.term_sig())
        }
    }))
}

/// Start the command after disconnecting, without waiting for it.
pub(super) fn after(command: &str, connection: &Connection) {
    if let Err(e) = spawn(command, connection, gio::SubprocessFlags::NONE) {
        tracing::warn!("the command after disconnecting: {e}");
    }
}

impl Session {
    /// Run the command before connecting, if there is one, then connect.
    pub(super) fn begin(&mut self, sender: &ComponentSender<Self>) {
        let command = self.connection.command_before.trim().to_owned();
        if command.is_empty() {
            return self.connect(sender);
        }
        self.clipboard = None;
        if let Some(handle) = self.handle.take() {
            handle.send(remote::Command::Close);
        }
        self.finished();
        self.page = Page::Connecting;
        self.connecting
            .set_title(&i18n("Running the Command Before Connecting…"));
        self.connecting.set_description(None);
        // A connection begun meanwhile makes this one stale.
        self.attempt += 1;
        let (attempt, connection, input) = (
            self.attempt,
            self.connection.clone(),
            sender.input_sender().clone(),
        );
        glib::spawn_future_local(async move {
            let result = before(command, connection).await;
            input.emit(SessionInput::Prepared(attempt, result));
        });
    }

    /// A connection that was begun is over: run the command after it.
    pub(super) fn finished(&mut self) {
        if !std::mem::take(&mut self.begun) {
            return;
        }
        let command = self.connection.command_after.trim();
        if !command.is_empty() {
            after(command, &self.connection);
        }
    }
}
