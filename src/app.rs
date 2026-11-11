//! The main window, laid out like Hylki's: a sidebar with the app's name and
//! main menu above it, New Connection and the connections beneath, and the
//! chosen connection beside it.

mod about;
mod actions;
mod details;
mod files;
mod groups;
mod sessions;
mod showcase;
mod sidebar;

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use relm4::prelude::*;

use crate::config::{self, ColorScheme, Settings};
use crate::connection::{Connection, split_address};
use crate::i18n::{i18n, i18n_f};
use crate::secrets::{self, Secret};
use crate::ui::editor::{Editor, EditorInit, EditorOutput};
use crate::ui::preferences::{PrefChange, Preferences};
use crate::ui::scan::{Scan, ScanInit, ScanMsg, ScanOutput};
use crate::ui::session::{Session, SessionOutput};
use crate::{APP_ID, APP_NAME};

pub(crate) struct AppModel {
    settings: Settings,
    connections: Vec<Connection>,
    selected: Option<String>,
    /// The sidebar search, lower case.
    query: String,
    list: gtk::ListBox,
    hint: gtk::Label,
    search: gtk::SearchEntry,
    search_bar: gtk::SearchBar,
    details: adw::Bin,
    toasts: adw::ToastOverlay,
    split: adw::OverlaySplitView,
    /// Set while the list is rebuilt, so its selection changes aren't the
    /// user's.
    quiet: Rc<Cell<bool>>,
    editor: Option<Controller<Editor>>,
    /// The Scan Network dialog, while it is open.
    scan: Option<Controller<Scan>>,
    /// The server the open editor is adding from that dialog.
    adding: Option<std::net::SocketAddrV4>,
    prefs: Option<Controller<Preferences>>,
    /// The content pane: the chosen connection, or a session's page.
    panes: gtk::Stack,
    /// Open sessions, by connection.
    sessions: HashMap<String, Controller<Session>>,
    /// The connections whose session is connected right now.
    live: HashSet<String>,
    /// The windows of the sessions moved out of this one.
    detached: HashMap<String, adw::ApplicationWindow>,
    /// The groups, for the sidebar's menus.
    groups: groups::SharedGroups,
}

#[derive(Debug)]
pub(crate) enum AppMsg {
    /// `host` fills the address in, from the search field.
    NewConnection(Option<String>),
    Select(String),
    Search(String),
    /// Enter in the search field.
    SearchActivated,
    FocusSearch,
    EditSelected,
    ConnectSelected,
    /// From a sidebar row's own buttons and menu: that connection, selected
    /// or not.
    Edit(String),
    Connect(String),
    Duplicate(String),
    Export(String),
    ConfirmDelete(String),
    Session(SessionOutput),
    /// End a session, as closing its window does.
    CloseSession(String),
    DuplicateSelected,
    DeleteSelected,
    /// Move a connection beside another, or to the end; dragged there, or
    /// Alt+Up and Alt+Down.
    Move {
        id: String,
        target: String,
        after: bool,
    },
    /// Put a connection in a group; an empty one for none.
    SetGroup {
        id: String,
        group: String,
    },
    /// Ask for a new group's name, for this connection.
    NewGroup(String),
    RenameGroup(String),
    /// Move a group's connections to another, or to none.
    Regroup {
        old: String,
        new: String,
    },
    Delete(String),
    Saved {
        connection: Box<Connection>,
        /// The passwords changed in the editor; an empty one is forgotten.
        passwords: Vec<(Secret, crate::secrets::Password)>,
    },
    EditorClosed,
    ImportRdp,
    ImportFiles(Vec<PathBuf>),
    ImportRemmina,
    ScanNetwork,
    /// A server the scan found, to open the editor on.
    AddFound(Box<crate::scan::Found>),
    /// The networks Scan Network covers besides this computer's.
    ScanNetworks(Vec<String>),
    ScanClosed,
    ExportSelected,
    ExportTo(String, PathBuf),
    OpenSettings,
    Pref(PrefChange),
    About,
    Shortcuts,
    ShowcaseSettingsPage(String),
    /// Scan Network's networks, with this text entered, if any.
    ShowcaseScanNetworks(String),
    ShowcaseSession(String),
    ShowcaseCapture(String),
}

#[derive(Debug)]
pub(crate) enum AppCmd {
    Keyring(Result<(), String>),
}

#[relm4::component(pub(crate))]
impl Component for AppModel {
    type Init = ();
    type Input = AppMsg;
    type Output = ();
    type CommandOutput = AppCmd;

    view! {
        adw::ApplicationWindow {
            set_title: Some(APP_NAME),
            set_icon_name: Some(APP_ID),
            set_default_size: (window.width, window.height),
            set_maximized: window.maximized,
            // Only the size is kept: on Wayland the compositor places windows.
            connect_close_request => move |w| {
                save_window_state(w);
                gtk::glib::Propagation::Proceed
            },

            #[local_ref]
            toasts -> adw::ToastOverlay {
                #[local_ref]
                #[wrap(Some)]
                set_child = split -> adw::OverlaySplitView {
                    set_min_sidebar_width: 220.0,
                    set_max_sidebar_width: 300.0,
                    set_sidebar_width_fraction: 0.25,

                    #[wrap(Some)]
                    set_sidebar = &adw::ToolbarView {
                        add_top_bar = &adw::HeaderBar {
                            add_css_class: "flat",
                            #[wrap(Some)]
                            set_title_widget = &gtk::Label {
                                set_label: APP_NAME,
                                add_css_class: "app-title",
                            },
                            pack_start = &gtk::Button {
                                set_icon_name: "list-add-symbolic",
                                set_tooltip_text: Some(&i18n("New Connection")),
                                update_property: &[gtk::accessible::Property::Label(&i18n("New Connection"))],
                                set_action_name: Some("win.new-connection"),
                            },
                            pack_start = &gtk::MenuButton {
                                set_icon_name: "view-sort-descending-symbolic",
                                set_tooltip_text: Some(&i18n("Sort By")),
                                update_property: &[gtk::accessible::Property::Label(&i18n("Sort By"))],
                                set_menu_model: Some(&sort_menu),
                            },
                            pack_end = &gtk::MenuButton {
                                set_icon_name: "open-menu-symbolic",
                                set_tooltip_text: Some(&i18n("Main Menu")),
                                update_property: &[gtk::accessible::Property::Label(&i18n("Main Menu"))],
                                set_primary: true,
                                add_css_class: "flat",
                                set_menu_model: Some(&main_menu),
                            },
                            // pack_end packs right to left: just left of the menu.
                            #[name = "search_button"]
                            pack_end = &gtk::ToggleButton {
                                set_icon_name: "system-search-symbolic",
                                set_tooltip_text: Some(&i18n("Search")),
                                update_property: &[gtk::accessible::Property::Label(&i18n("Search"))],
                            },
                        },

                        // Hidden until the search button or Ctrl+F; Escape
                        // closes it and clears the search.
                        #[local_ref]
                        add_top_bar = search_bar -> gtk::SearchBar {
                            #[local_ref]
                            #[wrap(Some)]
                            set_child = search -> gtk::SearchEntry {
                                set_hexpand: true,
                                set_placeholder_text: Some(&i18n("Search or Enter an Address")),
                                connect_search_changed[sender] => move |e| {
                                    sender.input(AppMsg::Search(e.text().into()));
                                },
                                connect_activate => AppMsg::SearchActivated,
                            },
                        },

                        #[wrap(Some)]
                        set_content = &gtk::Box {
                            set_orientation: gtk::Orientation::Vertical,

                            gtk::ScrolledWindow {
                                set_vexpand: true,
                                set_hscrollbar_policy: gtk::PolicyType::Never,

                                #[wrap(Some)]
                                set_child = &gtk::Overlay {
                                    #[wrap(Some)]
                                    set_child = &gtk::Box {
                                        set_orientation: gtk::Orientation::Vertical,
                                        #[local_ref]
                                        list -> gtk::ListBox {},
                                        #[local_ref]
                                        hint -> gtk::Label {},
                                    },
                                    add_overlay: drop_line,
                                },
                            },
                        },
                    },

                    // The chosen connection, or the session open to it.
                    #[local_ref]
                    #[wrap(Some)]
                    set_content = panes -> gtk::Stack {
                    add_named[Some("browse")] = &adw::ToolbarView {
                        add_top_bar = &adw::HeaderBar {
                            add_css_class: "flat",
                            // The connection names itself in the page below.
                            #[wrap(Some)]
                            set_title_widget = &gtk::Label {},
                            pack_start = &gtk::Button {
                                set_icon_name: "sidebar-show-symbolic",
                                set_tooltip_text: Some(&i18n("Toggle Sidebar")),
                                update_property: &[gtk::accessible::Property::Label(&i18n("Toggle Sidebar"))],
                                connect_clicked[split = split.clone()] => move |_| {
                                    split.set_show_sidebar(!split.shows_sidebar());
                                },
                            },
                            pack_end = &gtk::MenuButton {
                                set_icon_name: "view-more-symbolic",
                                set_tooltip_text: Some(&i18n("Connection Menu")),
                                update_property: &[gtk::accessible::Property::Label(&i18n("Connection Menu"))],
                                set_menu_model: Some(&connection_menu),
                                #[watch]
                                set_visible: model.selected.is_some(),
                            },
                            pack_end = &gtk::Button {
                                set_icon_name: "document-edit-symbolic",
                                set_tooltip_text: Some(&i18n("Edit Connection")),
                                update_property: &[gtk::accessible::Property::Label(&i18n("Edit Connection"))],
                                set_action_name: Some("win.edit"),
                                #[watch]
                                set_visible: model.selected.is_some(),
                            },
                        },

                        #[wrap(Some)]
                        set_content = &gtk::Stack {
                            add_named[Some("empty")] = &adw::StatusPage {
                                set_icon_name: Some("co.hyprlab.Mote-symbolic"),
                                set_title: &i18n("No Connections"),
                                set_description: Some(&i18n("Add a connection, or import one from an .rdp file or from Remmina.")),
                                #[wrap(Some)]
                                set_child = &gtk::Box {
                                    set_orientation: gtk::Orientation::Vertical,
                                    set_halign: gtk::Align::Center,
                                    set_spacing: 12,
                                    gtk::Button {
                                        set_label: &i18n("New Connection"),
                                        add_css_class: "pill",
                                        add_css_class: "suggested-action",
                                        set_action_name: Some("win.new-connection"),
                                    },
                                    gtk::Button {
                                        set_label: &i18n("Scan Network"),
                                        add_css_class: "pill",
                                        set_action_name: Some("win.scan-network"),
                                    },
                                },
                            },
                            add_named[Some("none")] = &adw::StatusPage {
                                set_icon_name: Some("co.hyprlab.Mote-symbolic"),
                                set_title: &i18n("No Connection Selected"),
                            },
                            #[local_ref]
                            add_named[Some("details")] = details -> adw::Bin {},

                            // After the pages: naming one before it exists
                            // only earns a warning.
                            #[watch]
                            set_visible_child_name: model.content_page(),
                        },
                    },
                    },
                },
            },
        }
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        load_css();
        // Files pasted from a remote desktop in an earlier run have been
        // copied wherever they were pasted by now.
        std::thread::spawn(|| {
            let _ = std::fs::remove_dir_all(crate::ui::session::fetched_files());
        });
        // Passwords an earlier Flatpak version kept in its sandbox.
        #[cfg(not(target_os = "macos"))]
        relm4::spawn(async {
            if let Err(e) = crate::secrets::move_from_sandbox().await {
                tracing::warn!("moving the sandbox's passwords: {e}");
            }
        });
        // Only X11 reads a window's own icon; Wayland goes by the app ID.
        gtk::Window::set_default_icon_name(crate::APP_ID);
        let settings = config::load_settings();
        apply_color_scheme(settings.color_scheme);
        crate::rdp::h264::choose(settings.decoder);
        let window = settings.window.clone();

        let list = &gtk::ListBox::new();
        list.add_css_class("navigation-sidebar");
        let hint = &gtk::Label::new(None);
        hint.set_wrap(true);
        hint.set_visible(false);
        hint.add_css_class("dim-label");
        hint.add_css_class("sidebar-hint");
        let quiet = Rc::new(Cell::new(false));
        let drop_line = &sidebar::drop_line();
        let groups = groups::SharedGroups::default();
        sidebar::connect_list(list, drop_line, &groups, &quiet, &sender);

        let model = AppModel {
            settings,
            connections: config::load_connections(),
            selected: None,
            query: String::new(),
            list: list.clone(),
            hint: hint.clone(),
            search: gtk::SearchEntry::new(),
            search_bar: gtk::SearchBar::new(),
            details: adw::Bin::new(),
            toasts: adw::ToastOverlay::new(),
            split: adw::OverlaySplitView::new(),
            quiet,
            editor: None,
            scan: None,
            adding: None,
            prefs: None,
            panes: gtk::Stack::new(),
            sessions: HashMap::new(),
            live: HashSet::new(),
            detached: HashMap::new(),
            groups,
        };
        let split = &model.split.clone();
        let panes = &model.panes.clone();
        let (search, search_bar, details, toasts) = (
            &model.search.clone(),
            &model.search_bar.clone(),
            &model.details.clone(),
            &model.toasts.clone(),
        );
        let main_menu = actions::main_menu();
        let sort_menu = actions::sort_menu();
        let connection_menu = actions::connection_menu();
        let widgets = view_output!();
        search_bar.connect_entry(search);
        search_bar
            .bind_property("search-mode-enabled", &widgets.search_button, "active")
            .bidirectional()
            .sync_create()
            .build();

        // Narrow, the sidebar slides over the connection instead.
        let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            640.0,
            adw::LengthUnit::Sp,
        ));
        narrow.add_setter(split, "collapsed", Some(&true.to_value()));
        root.add_breakpoint(narrow);

        // Fullscreen is for a session: the sidebar steps aside meanwhile.
        let sidebar_before = Cell::new(true);
        root.connect_fullscreened_notify(glib::clone!(
            #[weak]
            split,
            move |window| {
                if window.is_fullscreen() {
                    sidebar_before.set(split.shows_sidebar());
                    split.set_show_sidebar(false);
                } else {
                    split.set_show_sidebar(sidebar_before.get());
                }
            }
        ));
        actions::install(&root, &sender, model.settings.sort);
        model.rebuild_list();
        model.arm_showcase(&sender);
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>, root: &Self::Root) {
        match msg {
            AppMsg::NewConnection(host) => {
                let mut conn = self.settings.new_connection();
                if let Some(host) = host {
                    // A VNC address says so; anything else is RDP.
                    match host.strip_prefix("vnc://") {
                        Some(host) => {
                            conn.protocol = crate::connection::Protocol::Vnc;
                            conn.port = crate::connection::VNC_PORT;
                            conn.host = host.to_owned();
                        }
                        None => conn.host = host,
                    }
                }
                self.open_editor(conn, true, &sender, root);
            }
            AppMsg::Select(id) => self.select(Some(id)),
            AppMsg::Search(text) => {
                self.query = text.trim().to_lowercase();
                self.rebuild_list();
            }
            AppMsg::SearchActivated => self.search_activated(&sender),
            AppMsg::FocusSearch => {
                self.split.set_show_sidebar(true);
                self.search_bar.set_search_mode(true);
                self.search.grab_focus();
            }
            AppMsg::EditSelected => {
                if let Some(conn) = self.selected_connection().cloned() {
                    self.open_editor(conn, false, &sender, root);
                }
            }
            AppMsg::Edit(id) => {
                if let Some(conn) = self.connections.iter().find(|c| c.id == id).cloned() {
                    self.open_editor(conn, false, &sender, root);
                }
            }
            AppMsg::Connect(id) => self.connect(&id, &sender),
            AppMsg::ConnectSelected => {
                if let Some(id) = self.selected.clone() {
                    self.connect(&id, &sender);
                }
            }
            AppMsg::Session(output) => self.session_output(output, &sender),
            AppMsg::CloseSession(id) => self.end_session(&id),
            AppMsg::DuplicateSelected => {
                if let Some(id) = self.selected.clone() {
                    self.duplicate(&id, &sender);
                }
            }
            AppMsg::Duplicate(id) => self.duplicate(&id, &sender),
            AppMsg::DeleteSelected => {
                if let Some(conn) = self.selected_connection() {
                    files::confirm_delete(conn, root, &sender);
                }
            }
            AppMsg::ConfirmDelete(id) => {
                if let Some(conn) = self.connections.iter().find(|c| c.id == id) {
                    files::confirm_delete(conn, root, &sender);
                }
            }
            AppMsg::Export(id) => {
                if let Some(conn) = self.connections.iter().find(|c| c.id == id) {
                    files::choose_export_path(conn, root, &sender);
                }
            }
            AppMsg::Delete(id) => {
                // Asked from the editor, which has nothing left to edit.
                if let Some(editor) = &self.editor {
                    editor.widget().close();
                }
                // With its row gone, a session would have no way back to it.
                self.end_session(&id);
                if let Some(gone) = self.connections.iter().find(|c| c.id == id) {
                    crate::ui::avatar::delete_picture(&gone.picture);
                }
                self.connections.retain(|c| c.id != id);
                config::save_connections(&self.connections);
                sender.oneshot_command(async move {
                    AppCmd::Keyring(secrets::delete_passwords(id).await)
                });
                self.select(None);
                self.rebuild_list();
            }
            AppMsg::Saved {
                connection,
                passwords,
            } => {
                self.tell_scan_saved(&connection);
                self.save_connection(*connection, passwords, &sender);
            }
            AppMsg::EditorClosed => {
                self.editor = None;
                self.adding = None;
            }
            AppMsg::ImportRdp => files::choose_files(root, &sender),
            AppMsg::ImportFiles(paths) => self.import_files(paths),
            AppMsg::ImportRemmina => self.import_remmina(),
            AppMsg::ScanNetwork => self.open_scan(&sender, root),
            AppMsg::AddFound(found) => self.add_found(*found, &sender, root),
            AppMsg::ScanNetworks(networks) => {
                self.settings.scan_networks = networks;
                let disk = config::load_settings();
                self.settings.window = disk.window;
                self.settings.about_height = disk.about_height;
                config::save_settings(&self.settings);
            }
            AppMsg::ScanClosed => self.scan = None,
            AppMsg::ExportSelected => {
                if let Some(conn) = self.selected_connection() {
                    files::choose_export_path(conn, root, &sender);
                }
            }
            AppMsg::ExportTo(id, path) => self.export(&id, &path),
            AppMsg::OpenSettings => self.open_settings(&sender, root),
            AppMsg::Pref(change) => self.apply_pref(change),
            AppMsg::Move { id, target, after } => self.move_connection(&id, &target, after, root),
            AppMsg::SetGroup { id, group } => self.set_group(&id, &group),
            AppMsg::NewGroup(id) => self.ask_new_group(id, root, &sender),
            AppMsg::RenameGroup(old) => self.ask_rename_group(old, root, &sender),
            AppMsg::Regroup { old, new } => self.regroup(&old, &new),
            AppMsg::About => about::present(root),
            AppMsg::Shortcuts => actions::present_shortcuts(root),
            AppMsg::ShowcaseSettingsPage(page) => self.showcase_settings_page(&page),
            AppMsg::ShowcaseScanNetworks(text) => {
                if let Some(scan) = &self.scan {
                    scan.emit(ScanMsg::ShowNetworks);
                    for network in text.split('+').filter(|t| !t.is_empty()) {
                        scan.emit(ScanMsg::AddNetwork(network.to_owned()));
                    }
                }
            }
            AppMsg::ShowcaseSession(step) => self.showcase_session(&step),
            AppMsg::ShowcaseCapture(path) => self.showcase_capture(&path, root),
        }
    }

    fn update_cmd(&mut self, msg: Self::CommandOutput, _: ComponentSender<Self>, _: &Self::Root) {
        match msg {
            AppCmd::Keyring(Ok(())) => {}
            AppCmd::Keyring(Err(e)) => {
                tracing::warn!("keyring: {e}");
                self.toast(&i18n_f(
                    "The keyring could not be reached: {error}",
                    &[("error", &e)],
                ));
            }
        }
    }
}

impl AppModel {
    fn content_page(&self) -> &'static str {
        if self.connections.is_empty() {
            "empty"
        } else if self.selected.is_some() {
            "details"
        } else {
            "none"
        }
    }

    fn selected_connection(&self) -> Option<&Connection> {
        let id = self.selected.as_ref()?;
        self.connections.iter().find(|c| &c.id == id)
    }

    fn select(&mut self, id: Option<String>) {
        self.selected = id.filter(|id| self.connections.iter().any(|c| &c.id == id));
        self.refresh_details();
        // A connection with a session open here shows the session.
        let page = self
            .selected
            .as_deref()
            .and_then(|id| self.session_page(id))
            .unwrap_or_else(|| "browse".to_owned());
        self.panes.set_visible_child_name(&page);
        self.highlight_selected();
        // Narrow, the sidebar covers the connection; get it out of the way.
        if self.selected.is_some() && self.split.is_collapsed() {
            self.split.set_show_sidebar(false);
        }
    }

    fn refresh_details(&self) {
        match self.selected_connection() {
            Some(conn) => self
                .details
                .set_child(Some(&details::page(conn, self.settings.show_large_avatars))),
            None => self.details.set_child(None::<&gtk::Widget>),
        }
    }

    /// Enter in the search field: the first match, or a new connection to
    /// what was typed when it is an address and nothing matches.
    fn search_activated(&mut self, sender: &ComponentSender<Self>) {
        if let Some(id) = sidebar::first_visible(&self.list) {
            self.select(Some(id));
            self.rebuild_list();
        } else if let Some((host, port)) = split_address(&self.search.text()) {
            let scheme = if self.search.text().trim().starts_with("vnc://") {
                "vnc://"
            } else {
                ""
            };
            let typed = match port {
                Some(port) => format!("{scheme}{host}:{port}"),
                None => format!("{scheme}{host}"),
            };
            sender.input(AppMsg::NewConnection(Some(typed)));
        }
    }

    fn open_editor(
        &mut self,
        connection: Connection,
        is_new: bool,
        sender: &ComponentSender<Self>,
        root: &adw::ApplicationWindow,
    ) {
        if self.editor.is_some() {
            return;
        }
        let editor = Editor::builder()
            .launch(EditorInit {
                connection,
                is_new,
                groups: crate::ui::group::list(&self.connections),
            })
            .forward(sender.input_sender(), |out| match out {
                EditorOutput::Saved {
                    connection,
                    passwords,
                } => AppMsg::Saved {
                    connection,
                    passwords,
                },
                EditorOutput::Delete(id) => AppMsg::ConfirmDelete(id),
                EditorOutput::Closed => AppMsg::EditorClosed,
            });
        editor.widget().present(Some(root));
        self.editor = Some(editor);
    }

    fn open_scan(&mut self, sender: &ComponentSender<Self>, root: &adw::ApplicationWindow) {
        if self.scan.is_some() {
            return;
        }
        // Through a tunnel or a gateway the host is seen from the far end,
        // so a machine here of that name is another one.
        let saved = self
            .connections
            .iter()
            .filter(|c| !c.ssh_tunnel && !c.gateway)
            .flat_map(Connection::addresses)
            .collect();
        let named = self
            .settings
            .scan_networks
            .iter()
            .filter_map(|network| network.parse().ok())
            .collect();
        let scan = Scan::builder().launch(ScanInit { saved, named }).forward(
            sender.input_sender(),
            |out| match out {
                ScanOutput::Add(found) => AppMsg::AddFound(found),
                ScanOutput::Networks(networks) => AppMsg::ScanNetworks(networks),
                ScanOutput::Closed => AppMsg::ScanClosed,
            },
        );
        scan.widget().present(Some(root));
        self.scan = Some(scan);
    }

    /// The editor on a new connection to a server the scan found, over the
    /// scan's dialog, which stays open for the next one.
    fn add_found(
        &mut self,
        found: crate::scan::Found,
        sender: &ComponentSender<Self>,
        root: &adw::ApplicationWindow,
    ) {
        if self.editor.is_some() {
            return;
        }
        self.adding = Some(found.address);
        let conn = found.connection(self.settings.new_connection());
        self.open_editor(conn, true, sender, root);
    }

    fn tell_scan_saved(&mut self, connection: &Connection) {
        if let (Some(address), Some(scan)) = (self.adding.take(), &self.scan) {
            scan.emit(ScanMsg::Saved {
                address,
                host: connection.host.clone(),
                port: connection.port,
            });
        }
    }

    fn save_connection(
        &mut self,
        connection: Connection,
        passwords: Vec<(Secret, crate::secrets::Password)>,
        sender: &ComponentSender<Self>,
    ) {
        let id = connection.id.clone();
        let title = connection.title().to_owned();
        match self.connections.iter_mut().find(|c| c.id == id) {
            Some(existing) => *existing = connection,
            None => self.connections.push(connection),
        }
        config::save_connections(&self.connections);
        for (kind, password) in passwords {
            let (id, title) = (id.clone(), title.clone());
            sender.oneshot_command(async move {
                AppCmd::Keyring(secrets::set_password(id, kind, title, password).await)
            });
        }
        self.select(Some(id));
        self.rebuild_list();
    }

    fn duplicate(&mut self, id: &str, sender: &ComponentSender<Self>) {
        let Some(original) = self.connections.iter().find(|c| c.id == id) else {
            return;
        };
        let copy = Connection {
            id: crate::connection::new_id(),
            name: i18n_f("{name} (Copy)", &[("name", &original.title())]),
            last_used: 0,
            // Its own copy of the image, so deleting one keeps the other's.
            picture: crate::ui::avatar::copy_picture(&original.picture).unwrap_or_default(),
            ..original.clone()
        };
        let (from, to, title) = (original.id.clone(), copy.id.clone(), copy.name.clone());
        let from_id = from.clone();
        sender.oneshot_command(async move {
            AppCmd::Keyring(secrets::copy_passwords(from, to, title).await)
        });
        let id = copy.id.clone();
        // Beside its original, which matters in a custom order.
        let at = self
            .connections
            .iter()
            .position(|c| c.id == from_id)
            .map_or(self.connections.len(), |i| i + 1);
        self.connections.insert(at, copy);
        config::save_connections(&self.connections);
        self.select(Some(id));
        self.rebuild_list();
    }

    fn open_settings(&mut self, sender: &ComponentSender<Self>, root: &adw::ApplicationWindow) {
        let prefs = self.prefs.get_or_insert_with(|| {
            Preferences::builder()
                .transient_for(root)
                .launch(self.settings.clone())
                .forward(sender.input_sender(), AppMsg::Pref)
        });
        prefs.widget().present();
    }

    fn apply_pref(&mut self, change: PrefChange) {
        let mut redetail = false;
        let s = &mut self.settings;
        let relist = match change {
            PrefChange::ColorScheme(scheme) => {
                s.color_scheme = scheme;
                apply_color_scheme(scheme);
                false
            }
            PrefChange::Sort(order) => {
                s.sort = order;
                true
            }
            PrefChange::ShowGroups(on) => {
                s.show_groups = on;
                true
            }
            PrefChange::ShowHosts(on) => {
                s.show_hosts = on;
                true
            }
            PrefChange::ShowAvatars(on) => {
                s.show_avatars = on;
                true
            }
            PrefChange::ShowLargeAvatars(on) => {
                s.show_large_avatars = on;
                redetail = true;
                false
            }
            PrefChange::DefaultPort(port) => {
                s.default_port = port;
                false
            }
            PrefChange::DefaultUsername(name) => {
                s.default_username = name;
                false
            }
            PrefChange::DefaultDomain(domain) => {
                s.default_domain = domain;
                false
            }
            PrefChange::DefaultScaling(scaling) => {
                s.default_scaling = scaling;
                false
            }
            PrefChange::DefaultQuality(quality) => {
                s.default_quality = quality;
                false
            }
            PrefChange::DefaultClipboard(on) => {
                s.default_clipboard = on;
                false
            }
            PrefChange::DefaultSound(on) => {
                s.default_sound = on;
                false
            }
            PrefChange::DefaultMicrophone(on) => {
                s.default_microphone = on;
                false
            }
            PrefChange::DefaultFullscreen(on) => {
                s.default_fullscreen = on;
                false
            }
            PrefChange::DefaultNewWindow(on) => {
                s.default_new_window = on;
                false
            }
            // Sessions take these when they start.
            PrefChange::KeyboardLayout(id) => {
                s.keyboard_layout = id;
                false
            }
            PrefChange::GrabKeyboard(on) => {
                s.grab_keyboard = on;
                false
            }
            PrefChange::Reconnect(on) => {
                s.reconnect = on;
                false
            }
            PrefChange::GrabNotice(on) => {
                s.grab_notice = on;
                false
            }
            PrefChange::H264Warning(on) => {
                s.h264_warning = on;
                false
            }
            PrefChange::SizeNotice(on) => {
                s.size_notice = on;
                false
            }
            PrefChange::ClipboardNotice(on) => {
                s.clipboard_notice = on;
                false
            }
            PrefChange::Decoder(source) => {
                s.decoder = source;
                crate::rdp::h264::choose(source);
                false
            }
        };
        // The windows' sizes are saved by the windows as they close; keep
        // what is on disk rather than the sizes they opened at.
        let disk = config::load_settings();
        s.window = disk.window;
        s.about_height = disk.about_height;
        config::save_settings(s);
        if relist {
            self.rebuild_list();
        }
        if redetail {
            self.refresh_details();
        }
    }

    fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::new(text));
    }
}

fn save_window_state(window: &adw::ApplicationWindow) {
    let mut settings = config::load_settings();
    let (width, height) = window.default_size();
    settings.window = config::WindowState {
        width,
        height,
        maximized: window.is_maximized(),
    };
    config::save_settings(&settings);
}

fn apply_color_scheme(scheme: ColorScheme) {
    adw::StyleManager::default().set_color_scheme(match scheme {
        ColorScheme::System => adw::ColorScheme::Default,
        ColorScheme::Light => adw::ColorScheme::ForceLight,
        ColorScheme::Dark => adw::ColorScheme::ForceDark,
    });
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("styles.css"));
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}
