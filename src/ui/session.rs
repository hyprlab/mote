//! A session: asks for what the connection lacks, connects, shows the
//! remote desktop, and says plainly how it ended. It lives in the main
//! window's content pane, or in a window of its own once detached, and
//! brings its own header either way.

mod clipboard;
mod commands;
mod connecting;
mod controls;
mod display;
mod endings;
mod h264_help;
pub(crate) mod keyboard;
mod monitors;
mod pages;
mod resize_help;

pub(crate) use clipboard::fetched_files;
pub(crate) use pages::scaling_label;

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use relm4::prelude::*;

use crate::config;
use crate::connection::{Connection, Scaling};
use crate::i18n::{i18n, i18n_f};
use crate::remote::{self, Ending, Event};
use crate::secrets::{self, Password, Secret};
use display::Display;

/// What a session starts with: its connection, and the app's settings
/// for sessions.
pub(crate) struct SessionInit {
    pub(crate) connection: Connection,
    pub(crate) grab: bool,
    pub(crate) reconnect: bool,
    /// 0 follows this computer's layout.
    pub(crate) keyboard_layout: u32,
    /// Say, the first time, that the shortcuts are held back.
    pub(crate) grab_notice: bool,
    /// Say when the server sends no H.264.
    pub(crate) h264_warning: bool,
    /// Say when Desktop Sharing keeps its screen's size.
    pub(crate) size_notice: bool,
    /// Say, the first time, that the server can read the clipboard.
    pub(crate) clipboard_notice: bool,
}

pub(crate) struct Session {
    connection: Connection,
    reconnect: bool,
    keyboard_layout: u32,
    page: Page,
    /// The account to sign in with: the profile's, or what was typed.
    username: String,
    domain: String,
    password: Password,
    /// The passwords of the SSH server and the RD Gateway, if saved.
    ssh_password: Password,
    gateway_password: Password,
    /// Save the typed account once a session starts.
    remember: bool,
    handle: Option<remote::Handle>,
    clipboard: Option<clipboard::Sharing>,
    /// The address in the header: the one the session went to, when the
    /// connection has several.
    address: String,
    /// Counts connections, so a late event of an earlier one is ignored.
    attempt: u32,
    /// A connection was begun, and the command after disconnecting is due
    /// once it ends.
    begun: bool,
    /// In a window of its own rather than the main window.
    detached: bool,
    /// Whether the window holding it is fullscreen, shared with the
    /// pointer watch that shows the header at the screen's top edge.
    fullscreen: Rc<Cell<bool>>,
    /// Fullscreen, the header was shown for the keyboard: it stays until
    /// the desktop has the keyboard again.
    pinned: Rc<Cell<bool>>,
    /// The window whose fullscreen state is followed.
    watched: Option<gtk::Window>,
    /// The connection opens fullscreen and its page doesn't fill the
    /// monitor yet: the desktop is asked for at the fullscreen size, not the
    /// window's.
    fullscreen_pending: bool,
    scaling: Scaling,
    /// Counts size changes, so only the last of a quick run is sent.
    resizes: u32,
    display: Display,
    /// The windows on the other monitors, while the session takes them all.
    spread: Option<monitors::Spread>,
    connecting: adw::StatusPage,
    sign_in: pages::SignIn,
    ended: adw::StatusPage,
    /// The ended page's How to Fix, for a server without H.264.
    help: gtk::Button,
    /// What the session learns of the computer, for that advice.
    findings: h264_help::Findings,
    h264_warning: bool,
    /// The toast about Desktop Sharing keeping its size: waiting to see
    /// whether the size sticks, and shown.
    size_checking: bool,
    size_told: bool,
    clipboard_notice: bool,
    stack: gtk::Stack,
    toasts: adw::ToastOverlay,
    menu: gio::Menu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    SignIn,
    Connecting,
    Desktop,
    Ended,
}

impl Page {
    fn name(self) -> &'static str {
        match self {
            Self::SignIn => "sign-in",
            Self::Connecting => "connecting",
            Self::Desktop => "desktop",
            Self::Ended => "ended",
        }
    }
}

#[derive(Debug)]
pub(crate) enum SessionInput {
    /// Connect with what the sign-in page holds.
    SignIn,
    /// Connect again, running the command before connecting first.
    Reconnect,
    /// Connect, once there is a size to ask for.
    Connect,
    /// The command before connecting finished.
    Prepared(u32, Result<(), String>),
    /// End the session and close it.
    Disconnect,
    ToggleFullscreen,
    /// Ask to move into a window of its own, or back.
    ToggleDetached,
    /// Where it is now, as the main window placed it.
    SetDetached(bool),
    /// Its window was mapped, perhaps a different one than before.
    Mapped,
    FullscreenChanged(bool),
    /// Stop waiting for the window to go fullscreen, which the compositor
    /// may refuse.
    FullscreenWaited,
    SetScaling(Scaling),
    SetGrab(bool),
    SetViewOnly(bool),
    /// Right Ctrl gave the keyboard back: to the session's menu button.
    LetGo,
    /// Go fullscreen on every monitor.
    AllMonitors,
    /// Back to the session window's own monitor alone.
    OneMonitor,
    /// Send a combination from the Send Keys menu, by its name there.
    SendKeys(String),
    Screenshot,
    /// The room for the desktop, or the screen's scale, changed.
    Resized,
    /// Send the size, unless a newer change came since.
    SendSize(u32),
    /// Show what to install for the server to send H.264.
    H264Help,
    /// What the computer runs, as chosen there.
    Chosen(h264_help::Choice),
    /// The server still keeps its size, a while after it first did.
    SizeKept,
    /// Show how to make Desktop Sharing resize with the window.
    ResizeHelp,
}

#[derive(Debug)]
pub(crate) enum SessionOutput {
    /// A session started; the connection was used just now.
    Connected(String),
    /// The account typed at sign-in worked, and is to be kept.
    Remember {
        id: String,
        username: String,
        domain: String,
        password: Password,
    },
    /// The session ended without the window closing: not connected now.
    Stopped(String),
    Closed(String),
    ToggleSidebar,
    /// The scaling mode was changed, to be kept.
    Scaling(String, Scaling),
    /// View-only was turned on or off, to be kept.
    ViewOnly(String, bool),
    /// The connection's clipboard notice was shown, not to show again.
    ClipboardTold(String),
    /// What the computer runs was chosen, to be kept.
    System {
        id: String,
        server: Option<crate::rdp::server::Server>,
        distro: Option<crate::rdp::server::Distro>,
        graphics: Option<crate::rdp::server::Graphics>,
    },
    Detach(String),
    Attach(String),
}

pub(crate) enum SessionCmd {
    /// The saved passwords, the account's first, `None` where there is none.
    Passwords(Result<[Option<Password>; 3], String>),
    Event(u32, Event),
    Screenshot(Result<std::path::PathBuf, String>),
}

impl std::fmt::Debug for SessionCmd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Passwords(_) => "Passwords",
            Self::Event(..) => "Event",
            Self::Screenshot(_) => "Screenshot",
        })
    }
}

#[relm4::component(pub(crate))]
impl Component for Session {
    type Init = SessionInit;
    type Input = SessionInput;
    type Output = SessionOutput;
    type CommandOutput = SessionCmd;

    view! {
        adw::ToolbarView {
            connect_map => SessionInput::Mapped,

            add_top_bar = &adw::HeaderBar {
                // Not in fullscreen, where the header is the session's alone:
                // its close button would look like the session's own, and
                // close the window instead.
                #[watch]
                set_show_end_title_buttons: !model.fullscreen.get(),
                #[watch]
                set_show_start_title_buttons: !model.fullscreen.get(),
                #[wrap(Some)]
                set_title_widget = &adw::WindowTitle {
                    set_title: model.connection.title(),
                    #[watch]
                    set_subtitle: &model.address,
                },
                pack_start = &gtk::Button {
                    set_icon_name: "sidebar-show-symbolic",
                    set_tooltip_text: Some(&i18n("Toggle Sidebar")),
                    update_property: &[gtk::accessible::Property::Label(&i18n("Toggle Sidebar"))],
                    connect_clicked[sender] => move |_| {
                        let _ = sender.output(SessionOutput::ToggleSidebar);
                    },
                    #[watch]
                    set_visible: !model.detached && !model.fullscreen.get(),
                },
                pack_end = &gtk::MenuButton {
                    set_icon_name: "view-more-symbolic",
                    set_tooltip_text: Some(&i18n("Session Menu")),
                    update_property: &[gtk::accessible::Property::Label(&i18n("Session Menu"))],
                    #[watch]
                    set_menu_model: Some(&model.menu),
                },
                pack_end = &gtk::Button {
                    #[watch]
                    set_icon_name: if model.fullscreen.get() {
                        "view-restore-symbolic"
                    } else {
                        "view-fullscreen-symbolic"
                    },
                    #[watch]
                    set_tooltip_text: Some(&if model.fullscreen.get() {
                        i18n("Leave Fullscreen")
                    } else {
                        i18n("Fullscreen")
                    }),
                    #[watch]
                    update_property: &[gtk::accessible::Property::Label(&if model.fullscreen.get() {
                        i18n("Leave Fullscreen")
                    } else {
                        i18n("Fullscreen")
                    })],
                    connect_clicked => SessionInput::ToggleFullscreen,
                },
                pack_end = &gtk::Button {
                    set_icon_name: "window-new-symbolic",
                    set_tooltip_text: Some(&i18n("Open in New Window")),
                    update_property: &[gtk::accessible::Property::Label(&i18n("Open in New Window"))],
                    connect_clicked => SessionInput::ToggleDetached,
                    #[watch]
                    set_visible: !model.detached,
                },
                // Kept to the left of the others, away from the window's
                // close button.
                pack_end = &gtk::Button {
                    set_icon_name: "media-playback-stop-symbolic",
                    set_tooltip_text: Some(&i18n("Disconnect")),
                    update_property: &[gtk::accessible::Property::Label(&i18n("Disconnect"))],
                    set_action_name: Some("session.disconnect"),
                },
            },

            #[local_ref]
            #[wrap(Some)]
            set_content = toasts -> adw::ToastOverlay {
                #[local_ref]
                #[wrap(Some)]
                set_child = stack -> gtk::Stack {
                    set_transition_type: gtk::StackTransitionType::Crossfade,
                    add_named: (sign_in_page, Some("sign-in")),
                    add_named: (connecting, Some("connecting")),
                    // Scrolls only at Actual Size, when the desktop is larger
                    // than the window.
                    add_named: (&gtk::ScrolledWindow::builder().child(display).build(), Some("desktop")),
                    add_named: (ended, Some("ended")),
                    #[watch]
                    set_visible_child_name: model.page.name(),
                },
            },
        }
    }

    fn init(
        init: SessionInit,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let connection = init.connection;
        let sign_in = pages::SignIn::new(&connection, &sender);
        let (ended, help) = pages::ended(&sender);
        let model = Session {
            reconnect: init.reconnect,
            keyboard_layout: init.keyboard_layout,
            username: connection.username.clone(),
            domain: connection.domain.clone(),
            password: Password::default(),
            ssh_password: Password::default(),
            gateway_password: Password::default(),
            remember: false,
            page: Page::Connecting,
            handle: None,
            clipboard: None,
            address: connection.address(),
            attempt: 0,
            begun: false,
            detached: false,
            fullscreen: Rc::new(Cell::new(false)),
            pinned: Rc::new(Cell::new(false)),
            watched: None,
            fullscreen_pending: connection.fullscreen,
            scaling: connection.scaling,
            resizes: 0,
            display: Display::default(),
            spread: None,
            connecting: adw::StatusPage::new(),
            sign_in,
            ended,
            help,
            findings: h264_help::Findings::default(),
            h264_warning: init.h264_warning,
            size_checking: false,
            // Turned off in Settings: as if already shown.
            size_told: !init.size_notice,
            clipboard_notice: init.clipboard_notice,
            stack: gtk::Stack::new(),
            toasts: adw::ToastOverlay::new(),
            menu: controls::menu(false),
            connection,
        };
        controls::install_actions(
            &root,
            &sender,
            model.scaling,
            init.grab,
            model.connection.view_only,
        );
        model.display.set_mode(model.scaling);
        model.display.set_grab(init.grab);
        model.display.set_view_only(model.connection.view_only);
        let input = sender.input_sender().clone();
        model
            .display
            .set_on_let_go(move || input.emit(SessionInput::LetGo));
        model
            .display
            .update_property(&[gtk::accessible::Property::Label(&i18n_f(
                "Desktop of {name}",
                &[("name", &model.connection.title())],
            ))]);
        let (toasts, told) = (model.toasts.clone(), Cell::new(!init.grab_notice));
        model.display.set_on_grabbed(move || {
            if !told.replace(true) {
                toasts.add_toast(adw::Toast::new(&i18n(
                    "Keys and shortcuts go to the remote desktop. Right Ctrl lets go of them.",
                )));
            }
        });
        let input = sender.input_sender().clone();
        model
            .display
            .set_on_size(move || input.emit(SessionInput::Resized));
        reveal_at_top_edge(&root, &model.fullscreen, &model.pinned);
        // A header shown for the keyboard hides once the desktop has it
        // again.
        model.display.connect_has_focus_notify(glib::clone!(
            #[weak]
            root,
            #[strong(rename_to = pinned)]
            model.pinned,
            #[strong(rename_to = fullscreen)]
            model.fullscreen,
            move |display| {
                if display.has_focus() && pinned.replace(false) && fullscreen.get() {
                    root.set_reveal_top_bars(false);
                }
            }
        ));
        pages::connecting(&model.connecting);
        let (stack, toasts, display, ended, connecting) = (
            &model.stack.clone(),
            &model.toasts.clone(),
            &model.display.clone(),
            &model.ended.clone(),
            &model.connecting.clone(),
        );
        let sign_in_page = &model.sign_in.page.clone();
        let widgets = view_output!();
        // The desktop takes the keyboard as soon as it shows: asked for
        // while its page is still hidden, GTK ignores the request.
        let display = model.display.clone();
        model.stack.connect_visible_child_notify(move |stack| {
            if stack.visible_child_name().as_deref() == Some("desktop") {
                display.grab_focus();
            }
        });

        let id = model.connection.id.clone();
        sender.oneshot_command(async move {
            let mut saved = [None, None, None];
            for (slot, kind) in saved.iter_mut().zip(Secret::ALL) {
                match secrets::password(id.clone(), kind).await {
                    Ok(password) => *slot = password,
                    Err(e) => return SessionCmd::Passwords(Err(e)),
                }
            }
            SessionCmd::Passwords(Ok(saved))
        });
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>, root: &Self::Root) {
        match msg {
            SessionInput::SignIn => {
                let account = self.sign_in.account();
                if account.username.trim().is_empty() {
                    self.sign_in.focus();
                    return;
                }
                self.username = account.username;
                self.domain = account.domain;
                self.password = account.password;
                self.remember = account.remember;
                self.begin(&sender);
            }
            SessionInput::Reconnect => self.begin(&sender),
            SessionInput::Connect => self.connect(&sender),
            SessionInput::Prepared(attempt, result) if attempt == self.attempt => match result {
                Ok(()) => self.connect(&sender),
                Err(error) => {
                    tracing::warn!("the command before connecting failed: {error}");
                    self.ended.set_title(&i18n("Could Not Connect"));
                    self.ended
                        .set_description(Some(&glib::markup_escape_text(&i18n_f(
                            "The command before connecting failed: {error}",
                            &[("error", &error)],
                        ))));
                    self.page = Page::Ended;
                }
            },
            SessionInput::Prepared(..) => {}
            SessionInput::Disconnect => {
                self.finished();
                if let Some(spread) = self.spread.take() {
                    spread.close();
                }
                self.clipboard = None;
                if let Some(handle) = self.handle.take() {
                    handle.send(remote::Command::Close);
                }
                self.leave_fullscreen(root);
                let _ = sender.output(SessionOutput::Closed(self.connection.id.clone()));
            }
            SessionInput::ToggleFullscreen => {
                if let Some(window) = window_of(root) {
                    if self.fullscreen.get() {
                        window.unfullscreen();
                    } else {
                        window.fullscreen();
                        self.display.grab_focus();
                    }
                }
            }
            SessionInput::ToggleDetached => {
                // A window leaving fullscreen first comes back as it was.
                self.leave_fullscreen(root);
                let id = self.connection.id.clone();
                let _ = sender.output(if self.detached {
                    SessionOutput::Attach(id)
                } else {
                    SessionOutput::Detach(id)
                });
            }
            SessionInput::SetDetached(detached) => {
                self.detached = detached;
                self.menu = controls::menu(detached);
                if self.page == Page::Desktop {
                    self.display.grab_focus();
                }
            }
            SessionInput::Mapped => self.watch_window(root, &sender),
            SessionInput::SetScaling(scaling) => {
                self.scaling = scaling;
                self.display.set_mode(scaling);
                let _ = sender.output(SessionOutput::Scaling(self.connection.id.clone(), scaling));
                sender.input(SessionInput::Resized);
            }
            SessionInput::SetGrab(grab) => {
                self.display.set_grab(grab);
                for display in self.spread.iter().flat_map(|s| s.displays()) {
                    display.set_grab(grab);
                }
            }
            SessionInput::AllMonitors => self.spread_out(&sender, root),
            SessionInput::LetGo => {
                tracing::debug!("Right Ctrl let go of the keyboard");
                // Fullscreen, the header shows, so the keyboard has somewhere
                // to be.
                if self.fullscreen.get() {
                    self.pinned.set(true);
                    root.set_reveal_top_bars(true);
                }
                if let Some(button) = session_menu(root) {
                    button.grab_focus();
                }
            }
            SessionInput::OneMonitor => self.gather(&sender),
            SessionInput::SetViewOnly(view_only) => {
                self.display.set_view_only(view_only);
                for display in self.spread.iter().flat_map(|s| s.displays()) {
                    display.set_view_only(view_only);
                }
                let id = self.connection.id.clone();
                let _ = sender.output(SessionOutput::ViewOnly(id, view_only));
            }
            SessionInput::SendKeys(name) => {
                if let Some(codes) = controls::keys(&name) {
                    self.display.send_keys(&codes);
                }
            }
            SessionInput::H264Help => {
                let input = sender.input_sender().clone();
                h264_help::present(
                    root,
                    self.connection.title(),
                    self.page == Page::Ended,
                    self.findings.choice(&self.connection),
                    move |choice| input.emit(SessionInput::Chosen(choice)),
                );
            }
            SessionInput::SizeKept => {
                self.size_checking = false;
                if self.display.keeps_size()
                    && resize_help::applies(self.findings.choice(&self.connection).server)
                    && !std::mem::replace(&mut self.size_told, true)
                {
                    let input = sender.input_sender().clone();
                    self.toasts.add_toast(resize_help::toast(move || {
                        input.emit(SessionInput::ResizeHelp);
                    }));
                }
            }
            SessionInput::ResizeHelp => resize_help::present(root, self.connection.title()),
            SessionInput::Chosen(choice) => {
                let c = &mut self.connection;
                if (c.server, c.distro, c.graphics)
                    != (choice.server, choice.distro, choice.graphics)
                {
                    (c.server, c.distro, c.graphics) =
                        (choice.server, choice.distro, choice.graphics);
                    let _ = sender.output(SessionOutput::System {
                        id: c.id.clone(),
                        server: choice.server,
                        distro: choice.distro,
                        graphics: choice.graphics,
                    });
                }
            }
            SessionInput::Screenshot => {
                let Some(texture) = self
                    .display
                    .texture()
                    .filter(|_| self.page == Page::Desktop)
                else {
                    return;
                };
                let path = controls::screenshot_path(self.connection.title());
                sender.oneshot_command(async move {
                    SessionCmd::Screenshot(controls::save_screenshot(texture, path).await)
                });
            }
            SessionInput::Resized => {
                // A window being dragged to a new size changes it many
                // times a second; the server hears the size it settles on.
                self.resizes = self.resizes.wrapping_add(1);
                let (input, resizes) = (sender.input_sender().clone(), self.resizes);
                glib::timeout_add_local_once(std::time::Duration::from_millis(300), move || {
                    input.emit(SessionInput::SendSize(resizes));
                });
            }
            SessionInput::SendSize(resizes) => {
                if resizes == self.resizes
                    && self.spread.is_none()
                    && self.scaling == Scaling::Resize
                    && self.page == Page::Desktop
                    && let (Some(handle), Some((width, height, scale))) =
                        (&self.handle, self.desktop_size())
                {
                    self.display.set_asked(width, height);
                    handle.send(remote::Command::Resize {
                        width: width.into(),
                        height: height.into(),
                        scale,
                    });
                }
            }
            SessionInput::FullscreenWaited => self.fullscreen_pending = false,
            SessionInput::FullscreenChanged(fullscreen) => {
                self.fullscreen.set(fullscreen);
                self.pinned.set(false);
                if !fullscreen {
                    self.gather(&sender);
                }
                root.set_reveal_top_bars(!fullscreen);
                // Shown at the top edge, the header covers the desktop
                // rather than pushing it down.
                root.set_extend_content_to_top_edge(fullscreen);
                root.set_top_bar_style(if fullscreen {
                    adw::ToolbarStyle::Raised
                } else {
                    adw::ToolbarStyle::Flat
                });
            }
        }
    }

    /// Closed with its window, or with the app: the session ends here too.
    fn shutdown(&mut self, _: &mut Self::Widgets, _: relm4::Sender<Self::Output>) {
        self.finished();
    }

    fn update_cmd(
        &mut self,
        msg: Self::CommandOutput,
        sender: ComponentSender<Self>,
        root: &Self::Root,
    ) {
        match msg {
            SessionCmd::Passwords(Ok([account, ssh, gateway])) => {
                self.ssh_password = ssh.unwrap_or_default();
                self.gateway_password = gateway.unwrap_or_default();
                match account {
                    Some(password) if !self.connection.username.trim().is_empty() => {
                        self.password = password;
                        self.begin(&sender);
                    }
                    _ => {
                        self.page = Page::SignIn;
                        self.sign_in.focus();
                    }
                }
            }
            SessionCmd::Passwords(Err(e)) => {
                tracing::warn!("reading the saved passwords: {e}");
                self.page = Page::SignIn;
                self.sign_in.focus();
            }
            SessionCmd::Event(attempt, event) if attempt == self.attempt => {
                self.event(event, &sender, root);
            }
            SessionCmd::Event(..) => {}
            SessionCmd::Screenshot(Ok(path)) => {
                let toast = adw::Toast::new(&i18n("Screenshot taken"));
                toast.set_button_label(Some(&i18n("Show")));
                let window = window_of(root);
                toast.connect_button_clicked(move |_| {
                    gtk::FileLauncher::new(Some(&gio::File::for_path(&path)))
                        .open_containing_folder(window.as_ref(), None::<&gio::Cancellable>, |_| {});
                });
                self.toasts.add_toast(toast);
            }
            SessionCmd::Screenshot(Err(e)) => {
                tracing::warn!("saving a screenshot: {e}");
                self.toasts
                    .add_toast(adw::Toast::new(&i18n("The screenshot could not be saved")));
            }
        }
    }
}

impl Session {
    /// Follow the fullscreen state of the window it is in, which changes
    /// when it moves between the main window and one of its own.
    fn watch_window(&mut self, root: &adw::ToolbarView, sender: &ComponentSender<Self>) {
        let Some(window) = window_of(root) else {
            return;
        };
        if self.watched.as_ref() == Some(&window) {
            return;
        }
        let input = sender.input_sender().clone();
        window.connect_fullscreened_notify(move |window| {
            input.emit(SessionInput::FullscreenChanged(window.is_fullscreen()));
        });
        sender.input(SessionInput::FullscreenChanged(window.is_fullscreen()));
        if self.fullscreen_pending && self.watched.is_none() {
            window.fullscreen();
            let input = sender.input_sender().clone();
            glib::timeout_add_local_once(std::time::Duration::from_secs(2), move || {
                input.emit(SessionInput::FullscreenWaited);
            });
        }
        self.watched = Some(window);
    }

    fn leave_fullscreen(&self, root: &adw::ToolbarView) {
        if self.fullscreen.get()
            && let Some(window) = window_of(root)
        {
            window.unfullscreen();
        }
    }

    /// A server that answers a resize with a desktop of another size may
    /// just be slow, so it counts as keeping its size only if it still
    /// does a few seconds later.
    fn check_size_kept(&mut self, sender: &ComponentSender<Self>) {
        if self.display.keeps_size() && !self.size_told && !self.size_checking {
            self.size_checking = true;
            let input = sender.input_sender().clone();
            glib::timeout_add_local_once(std::time::Duration::from_secs(3), move || {
                input.emit(SessionInput::SizeKept);
            });
        }
    }

    /// The size to ask the server for, in the screen's own pixels, and the
    /// screen's scale in percent: the room the window has, so that one
    /// desktop pixel is one screen pixel. `None` until there is a size to
    /// go by. A session whose page isn't showing has none of its own, and
    /// takes its window's.
    fn desktop_size(&self) -> Option<(u16, u16, u32)> {
        let (w, h) = match (self.stack.width(), window_of(&self.stack)) {
            (0, Some(window)) if window.width() > 0 => {
                // Less the header the page would have.
                (window.width(), window.height() - 46)
            }
            (0, _) => return None,
            (w, _) => (w, self.stack.height()),
        };
        let device = display::device_scale(&self.stack);
        let pixels = |logical: i32| (f64::from(logical) * device).round() as i32;
        // Servers want an even width, and nothing smaller than 200.
        let w = (pixels(w).clamp(200, 8192) & !1) as u16;
        let h = pixels(h).clamp(200, 8192) as u16;
        let scale = ((device * 100.0).round() as u32).clamp(100, 500);
        Some((w, h, scale))
    }

    fn event(&mut self, event: Event, sender: &ComponentSender<Self>, root: &adw::ToolbarView) {
        match event {
            Event::Certificate {
                host,
                port,
                fingerprint,
                of,
                also,
                reply,
            } => {
                let known = also
                    .iter()
                    .chain([&(host.clone(), port)])
                    .filter_map(|(host, port)| config::trusted_fingerprint(host, *port))
                    .collect();
                self.check_certificate((&host, port), fingerprint, of, known, reply, root);
            }
            Event::Trying(address) => {
                self.connecting
                    .set_description(Some(&gtk::glib::markup_escape_text(&address)));
                self.address = address;
            }
            Event::Waking => self.connecting.set_title(&i18n_f(
                "Waking {name}…",
                &[("name", &self.connection.title())],
            )),
            Event::Reconnecting => {
                self.connecting.set_title(&i18n_f(
                    "Reconnecting to {name}…",
                    &[("name", &self.connection.title())],
                ));
                self.page = Page::Connecting;
            }
            Event::Connected { width, height } => {
                tracing::info!("connected, {width}×{height}");
                // A reactivation reports the desktop again; only a new
                // connection counts.
                if self.page != Page::Desktop {
                    let _ = sender.output(SessionOutput::Connected(self.connection.id.clone()));
                    self.tell_clipboard(sender);
                    // The window may have changed size while the session
                    // was away, and a layout of several monitors is gone.
                    sender.input(SessionInput::Resized);
                    if let (Some(spread), Some(handle)) = (&self.spread, &self.handle) {
                        handle.send(remote::Command::Layout(spread.layout.clone()));
                    }
                }
                self.page = Page::Desktop;
                self.display.grab_focus();
                if std::mem::take(&mut self.remember) {
                    let _ = sender.output(SessionOutput::Remember {
                        id: self.connection.id.clone(),
                        username: self.username.clone(),
                        domain: self.domain.clone(),
                        password: self.password.clone(),
                    });
                }
            }
            Event::Frame(frame) => {
                self.display.show_frame(frame);
                self.check_size_kept(sender);
                if let (Some(spread), Some(texture)) = (&self.spread, self.display.texture()) {
                    for display in spread.displays() {
                        display.show_texture(texture.clone());
                    }
                }
                // The next frame can come now, and replace this one if it
                // isn't painted yet: what is painted is the newest there is.
                if let Some(handle) = &self.handle {
                    handle.send(remote::Command::FrameDone);
                }
            }
            Event::Cursor(cursor) => {
                for display in self.spread.iter().flat_map(|s| s.displays()) {
                    display.set_remote_cursor(cursor.clone());
                }
                self.display.set_remote_cursor(cursor);
            }
            Event::KeepsSize => {
                self.display.set_keeps_size();
                self.check_size_kept(sender);
            }
            event @ (Event::Server(_)
            | Event::System(_)
            | Event::Graphics { .. }
            | Event::NoH264) => {
                if self.findings.learn(event) && self.h264_warning {
                    let toast =
                        adw::Toast::new(&i18n("The remote computer is not sending H.264 video"));
                    toast.set_button_label(Some(&i18n("How to Fix")));
                    toast.set_timeout(10);
                    let input = sender.input_sender().clone();
                    toast.connect_button_clicked(move |_| input.emit(SessionInput::H264Help));
                    self.toasts.add_toast(toast);
                }
            }
            Event::OneMonitor => {
                if self.spread.is_some() {
                    self.gather(sender);
                    self.toasts.add_toast(adw::Toast::new(&i18n(
                        "The remote desktop can only use one monitor",
                    )));
                }
            }
            Event::Clipboard(event) => {
                if let Some(clipboard) = &self.clipboard {
                    clipboard.event(event);
                }
            }
            Event::Ended(ending) => {
                self.finished();
                if let Some(spread) = self.spread.take() {
                    spread.close();
                    self.display.set_viewport(None);
                }
                self.display.report();
                self.clipboard = None;
                self.handle = None;
                self.ended(ending, sender);
            }
        }
    }

    fn ended(&mut self, ending: Ending, sender: &ComponentSender<Self>) {
        let was_connected = self.page == Page::Desktop;
        if !matches!(ending, Ending::Closed) {
            let _ = sender.output(SessionOutput::Stopped(self.connection.id.clone()));
        }
        match endings::shown(
            ending,
            &self.connection,
            was_connected,
            self.findings.offered,
        ) {
            endings::Shown::Nothing => {}
            endings::Shown::SignIn(message) => {
                self.page = Page::SignIn;
                self.sign_in.refused(&message);
            }
            endings::Shown::Page {
                title,
                description,
                help,
            } => {
                self.help.set_visible(help);
                self.ended.set_title(&title);
                self.ended
                    .set_description(Some(&gtk::glib::markup_escape_text(&description)));
                self.page = Page::Ended;
            }
        }
    }
}

/// The session's menu button, in its header.
fn session_menu(root: &adw::ToolbarView) -> Option<gtk::MenuButton> {
    let mut pending: Vec<gtk::Widget> = vec![root.clone().upcast()];
    while let Some(widget) = pending.pop() {
        if let Some(button) = widget.downcast_ref::<gtk::MenuButton>() {
            return Some(button.clone());
        }
        // Not into the desktop's pages, where no menu button is.
        if widget.is::<gtk::Stack>() {
            continue;
        }
        let mut child = widget.first_child();
        while let Some(c) = child {
            child = c.next_sibling();
            pending.push(c);
        }
    }
    None
}

fn window_of(widget: &impl IsA<gtk::Widget>) -> Option<gtk::Window> {
    widget.root().and_downcast::<gtk::Window>()
}

/// Fullscreen, the header hides; the pointer at the top edge brings it back
/// until the pointer moves well clear of it.
/// `pinned`: shown for the keyboard, it stays whatever the pointer does.
fn reveal_at_top_edge(
    root: &adw::ToolbarView,
    fullscreen: &Rc<Cell<bool>>,
    pinned: &Rc<Cell<bool>>,
) {
    let motion = gtk::EventControllerMotion::new();
    // Before the desktop sees the pointer, so the header shows over it.
    motion.set_propagation_phase(gtk::PropagationPhase::Capture);
    let (fullscreen, pinned) = (fullscreen.clone(), pinned.clone());
    motion.connect_motion(glib::clone!(
        #[weak]
        root,
        move |_, _, y| {
            if !fullscreen.get() {
                return;
            }
            if y <= 2.0 {
                root.set_reveal_top_bars(true);
            } else if !pinned.get()
                && root.reveals_top_bars()
                && y > f64::from(root.top_bar_height()) + 48.0
            {
                root.set_reveal_top_bars(false);
            }
        }
    ));
    root.add_controller(motion);
}
