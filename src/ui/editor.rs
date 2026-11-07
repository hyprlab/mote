//! The connection editor: a dialog for adding a connection or changing one.
//! Later sections add their settings to it as pages of their own.

use crate::ui::group::GroupRow;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use relm4::prelude::*;

use crate::connection::{
    Connection, Network, Protocol, Quality, Scaling, VncCodec, parse_mac, split_address,
};
use crate::i18n::i18n;
use crate::secrets::{self, Password, Secret};
use crate::ui::avatar;

mod addresses;

use addresses::{moved, other_addresses, other_row};

pub(crate) struct EditorInit {
    pub(crate) connection: Connection,
    pub(crate) is_new: bool,
    /// The groups in use, to choose from.
    pub(crate) groups: Vec<String>,
}

pub(crate) struct Editor {
    base: Connection,
    valid: bool,
    rows: Rows,
    /// The passwords the user typed in, never the keyring filling a row
    /// in: an untouched password is not written back.
    edited: Vec<Secret>,
    filling: Rc<Cell<bool>>,
    /// The robot or image chosen for the connection.
    choice: Rc<RefCell<avatar::Choice>>,
    saved: bool,
}

struct Rows {
    name: adw::EntryRow,
    host: adw::EntryRow,
    /// The other addresses: a row each, under `other`.
    other: adw::ExpanderRow,
    others: Vec<adw::EntryRow>,
    protocol: adw::ComboRow,
    port: adw::SpinRow,
    group: GroupRow,
    username: adw::EntryRow,
    domain: adw::EntryRow,
    password: adw::PasswordEntryRow,
    allow_unencrypted: adw::SwitchRow,
    mac: adw::EntryRow,
    ssh: adw::ExpanderRow,
    ssh_host: adw::EntryRow,
    ssh_username: adw::EntryRow,
    ssh_password: adw::PasswordEntryRow,
    gateway: adw::ExpanderRow,
    gateway_host: adw::EntryRow,
    gateway_username: adw::EntryRow,
    gateway_password: adw::PasswordEntryRow,
    scaling: adw::ComboRow,
    quality: adw::ComboRow,
    vnc_codec: adw::ComboRow,
    view_only: adw::SwitchRow,
    fullscreen: adw::SwitchRow,
    new_window: adw::SwitchRow,
    network: adw::ComboRow,
    clipboard: adw::SwitchRow,
    clipboard_files: adw::SwitchRow,
    sound: adw::SwitchRow,
    microphone: adw::SwitchRow,
    folder: adw::ActionRow,
    command_before: adw::EntryRow,
    command_after: adw::EntryRow,
    /// The folder chosen in `folder`, empty for none.
    folder_path: Rc<RefCell<String>>,
}

#[derive(Debug)]
pub(crate) enum EditorMsg {
    /// Something that decides whether the connection can be saved.
    Changed,
    PasswordEdited(Secret),
    AddAddress,
    RemoveAddress(adw::EntryRow),
    /// An address dragged to just before or after another.
    MoveAddress {
        row: adw::EntryRow,
        beside: adw::EntryRow,
        after: bool,
    },
    /// Alt+Up or Alt+Down on an address: past its neighbor.
    NudgeAddress {
        row: adw::EntryRow,
        down: bool,
    },
    /// The other addresses opened; with none yet, opening them is for a
    /// first one.
    OthersOpened,
    Save,
    Cancel,
    Delete,
}

#[derive(Debug)]
pub(crate) enum EditorOutput {
    /// `passwords` holds the ones changed; an empty one is forgotten.
    Saved {
        connection: Box<Connection>,
        passwords: Vec<(Secret, Password)>,
    },
    /// Delete was pressed, for the connection with this id. The question
    /// comes first, so this stays open until it is answered.
    Delete(String),
    Closed,
}

#[derive(Debug)]
pub(crate) enum EditorCmd {
    PasswordLoaded(Secret, Result<Option<Password>, String>),
}

#[relm4::component(pub(crate))]
impl Component for Editor {
    type Init = EditorInit;
    type Input = EditorMsg;
    type Output = EditorOutput;
    type CommandOutput = EditorCmd;

    view! {
        adw::Dialog {
            set_title: &title,
            set_content_width: 520,
            connect_closed[sender] => move |_| {
                let _ = sender.output(EditorOutput::Closed);
            },

            #[wrap(Some)]
            set_child = &adw::ToolbarView {
                add_top_bar = &adw::HeaderBar {
                    set_show_start_title_buttons: false,
                    set_show_end_title_buttons: false,
                    pack_start = &gtk::Button {
                        set_label: &i18n("Cancel"),
                        connect_clicked => EditorMsg::Cancel,
                    },
                    #[name = "save"]
                    pack_end = &gtk::Button {
                        set_label: &save_label,
                        add_css_class: "suggested-action",
                        #[watch]
                        set_sensitive: model.valid,
                        connect_clicked => EditorMsg::Save,
                    },
                    // Left of Save, as in Hylki's account editor; a new
                    // connection has nothing to delete.
                    pack_end = &gtk::Button {
                        set_label: &i18n("Delete…"),
                        add_css_class: "destructive-action",
                        set_visible: !is_new,
                        connect_clicked => EditorMsg::Delete,
                    },
                },
                set_content: Some(&page),
            },
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let c = &init.connection;
        let is_new = init.is_new;
        let (title, save_label) = if is_new {
            (i18n("New Connection"), i18n("Add"))
        } else {
            (i18n("Edit Connection"), i18n("Save"))
        };
        let mut rows = Rows {
            name: entry(&i18n("Name"), &c.name),
            host: entry(&i18n("Address"), &c.host),
            other: adw::ExpanderRow::new(),
            others: Vec::new(),
            protocol: adw::ComboRow::new(),
            port: adw::SpinRow::with_range(1.0, 65535.0, 1.0),
            group: GroupRow::new(&c.group, init.groups.clone()),
            username: entry(&i18n("User Name"), &c.username),
            domain: entry(&i18n("Domain"), &c.domain),
            password: password_entry(&i18n("Password")),
            allow_unencrypted: adw::SwitchRow::new(),
            mac: entry(&i18n("Wake-on-LAN MAC Address"), &c.mac_address),
            ssh: adw::ExpanderRow::new(),
            ssh_host: entry(&i18n("Address"), &c.ssh_host),
            ssh_username: entry(&i18n("User Name"), &c.ssh_username),
            ssh_password: password_entry(&i18n("Password")),
            gateway: adw::ExpanderRow::new(),
            gateway_host: entry(&i18n("Address"), &c.gateway_host),
            gateway_username: entry(&i18n("User Name"), &c.gateway_username),
            gateway_password: password_entry(&i18n("Password")),
            scaling: adw::ComboRow::new(),
            quality: adw::ComboRow::new(),
            vnc_codec: adw::ComboRow::new(),
            view_only: adw::SwitchRow::new(),
            fullscreen: adw::SwitchRow::new(),
            new_window: adw::SwitchRow::new(),
            network: adw::ComboRow::new(),
            clipboard: adw::SwitchRow::new(),
            clipboard_files: adw::SwitchRow::new(),
            sound: adw::SwitchRow::new(),
            microphone: adw::SwitchRow::new(),
            folder: adw::ActionRow::new(),
            command_before: entry(&i18n("Before Connecting"), &c.command_before),
            command_after: entry(&i18n("After Disconnecting"), &c.command_after),
            folder_path: Rc::new(RefCell::new(c.folder.clone())),
        };
        folder_row(&rows.folder, &rows.folder_path);
        let labels: Vec<&str> = Protocol::ALL.iter().map(|p| p.name()).collect();
        rows.protocol.set_title(&i18n("Protocol"));
        rows.protocol
            .set_model(Some(&gtk::StringList::new(&labels)));
        rows.protocol.set_selected(
            Protocol::ALL
                .iter()
                .position(|p| *p == c.protocol)
                .unwrap_or_default() as u32,
        );
        let labels: Vec<String> = Scaling::ALL
            .into_iter()
            .map(crate::ui::session::scaling_label)
            .collect();
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        rows.scaling.set_title(&i18n("Scaling"));
        rows.scaling.set_model(Some(&gtk::StringList::new(&labels)));
        rows.scaling.set_selected(
            Scaling::ALL
                .iter()
                .position(|s| *s == c.scaling)
                .unwrap_or_default() as u32,
        );
        let labels: Vec<String> = Quality::ALL.into_iter().map(quality_label).collect();
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        rows.quality.set_title(&i18n("Codec"));
        rows.quality.set_model(Some(&gtk::StringList::new(&labels)));
        rows.quality.connect_selected_notify(|row| {
            let quality = Quality::ALL.get(row.selected() as usize).copied();
            row.set_subtitle(&quality.map(quality_cost).unwrap_or_default());
        });
        rows.quality.set_selected(
            Quality::ALL
                .iter()
                .position(|q| *q == c.quality)
                .unwrap_or_default() as u32,
        );
        rows.quality.notify("selected");
        let labels: Vec<String> = VncCodec::ALL.into_iter().map(vnc_codec_label).collect();
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        rows.vnc_codec.set_title(&i18n("Codec"));
        rows.vnc_codec
            .set_model(Some(&gtk::StringList::new(&labels)));
        rows.vnc_codec.connect_selected_notify(|row| {
            let codec = VncCodec::ALL.get(row.selected() as usize).copied();
            row.set_subtitle(&codec.map(vnc_codec_cost).unwrap_or_default());
        });
        rows.vnc_codec.set_selected(
            VncCodec::ALL
                .iter()
                .position(|q| *q == c.vnc_codec)
                .unwrap_or_default() as u32,
        );
        rows.vnc_codec.notify("selected");
        let labels: Vec<String> = Network::ALL.into_iter().map(network_label).collect();
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        rows.network.set_title(&i18n("Connection Type"));
        rows.network.set_subtitle(&i18n("Only supported by xrdp"));
        rows.network.set_model(Some(&gtk::StringList::new(&labels)));
        rows.network.set_selected(
            Network::ALL
                .iter()
                .position(|n| *n == c.network)
                .unwrap_or_default() as u32,
        );
        rows.allow_unencrypted.set_title(&i18n("Allow Unencrypted"));
        rows.allow_unencrypted.set_active(c.allow_unencrypted);
        rows.view_only.set_title(&i18n("View Only"));
        rows.view_only.set_active(c.view_only);
        rows.fullscreen.set_title(&i18n("Open Fullscreen"));
        rows.fullscreen.set_active(c.fullscreen);
        rows.new_window.set_title(&i18n("Open in New Window"));
        rows.new_window.set_active(c.new_window);
        rows.clipboard.set_title(&i18n("Clipboard"));
        rows.clipboard.set_active(c.clipboard);
        rows.clipboard_files.set_title(&i18n("Share Copied Files"));
        rows.clipboard_files
            .set_subtitle(&i18n("The server can read any file copied here"));
        rows.clipboard_files.set_active(c.clipboard_files);
        rows.clipboard
            .bind_property("active", &rows.clipboard_files, "sensitive")
            .sync_create()
            .build();
        // Both depend on the server the same way.
        let audio_limit = i18n("Not with KRdp; xrdp needs its PipeWire module");
        rows.sound.set_title(&i18n("Sound"));
        rows.sound.set_subtitle(&audio_limit);
        rows.sound.set_active(c.sound);
        rows.microphone.set_title(&i18n("Microphone"));
        rows.microphone.set_subtitle(&audio_limit);
        rows.microphone.set_active(c.microphone);
        rows.port.set_title(&i18n("Port"));
        rows.port.set_value(f64::from(c.port));
        other_addresses(&rows.other, &sender);
        for address in &c.other_addresses {
            let row = other_row(address, &sender);
            rows.other.add_row(&row);
            rows.others.push(row);
        }
        rows.other.set_expanded(!rows.others.is_empty());
        let input = sender.input_sender().clone();
        rows.other.connect_expanded_notify(move |row| {
            if row.is_expanded() {
                input.emit(EditorMsg::OthersOpened);
            }
        });
        rows.ssh.set_title(&i18n("SSH Tunnel"));
        rows.ssh
            .set_subtitle(&i18n("A password is needed only if no SSH key works"));
        rows.ssh.set_show_enable_switch(true);
        rows.ssh.set_enable_expansion(c.ssh_tunnel);
        rows.ssh.set_expanded(false);
        for row in [
            rows.ssh_host.upcast_ref::<gtk::Widget>(),
            rows.ssh_username.upcast_ref(),
            rows.ssh_password.upcast_ref(),
        ] {
            rows.ssh.add_row(row);
        }
        rows.gateway.set_title(&i18n("RD Gateway"));
        rows.gateway.set_show_enable_switch(true);
        rows.gateway.set_enable_expansion(c.gateway);
        rows.gateway.set_expanded(false);
        for row in [
            rows.gateway_host.upcast_ref::<gtk::Widget>(),
            rows.gateway_username.upcast_ref(),
            rows.gateway_password.upcast_ref(),
        ] {
            rows.gateway.add_row(row);
        }
        // One or the other: a gateway can't be reached through the tunnel.
        let other = rows.gateway.clone();
        let username = rows.ssh_username.clone();
        rows.ssh.connect_enable_expansion_notify(move |ssh| {
            if ssh.enables_expansion() {
                other.set_enable_expansion(false);
                if username.text().trim().is_empty() {
                    username.set_text(&gtk::glib::user_name().to_string_lossy());
                }
            }
        });
        let other = rows.ssh.clone();
        rows.gateway
            .connect_enable_expansion_notify(move |gateway| {
                if gateway.enables_expansion() {
                    other.set_enable_expansion(false);
                }
            });

        for row in [&rows.host, &rows.ssh_host, &rows.gateway_host, &rows.mac] {
            let input = sender.input_sender().clone();
            row.connect_changed(move |_| input.emit(EditorMsg::Changed));
        }
        for row in [&rows.ssh, &rows.gateway] {
            let input = sender.input_sender().clone();
            row.connect_enable_expansion_notify(move |_| input.emit(EditorMsg::Changed));
        }
        let filling = Rc::new(Cell::new(false));
        for (row, kind) in [
            (&rows.password, Secret::Account),
            (&rows.ssh_password, Secret::Ssh),
            (&rows.gateway_password, Secret::Gateway),
        ] {
            let (input, quiet) = (sender.input_sender().clone(), filling.clone());
            row.connect_changed(move |_| {
                if !quiet.get() {
                    input.emit(EditorMsg::PasswordEdited(kind));
                }
            });
        }

        let page = adw::PreferencesPage::new();
        let choice = Rc::new(RefCell::new(avatar::Choice {
            robot: c.robot.clone(),
            picture: c.picture.clone(),
            imported: Vec::new(),
        }));
        let picture = adw::PreferencesGroup::new();
        picture.add(&avatar::picker(&c.id, &choice));
        page.add(&picture);
        let target = adw::PreferencesGroup::new();
        for row in [
            rows.name.upcast_ref::<gtk::Widget>(),
            rows.host.upcast_ref(),
            rows.other.upcast_ref(),
            rows.protocol.upcast_ref(),
            rows.port.upcast_ref(),
            rows.group.row.upcast_ref(),
        ] {
            target.add(row);
        }
        page.add(&target);
        let account = adw::PreferencesGroup::new();
        account.set_title(&i18n("Account"));
        account.set_description(Some(&i18n(
            "Leave the password empty to be asked for it when connecting.",
        )));
        for row in [
            rows.username.upcast_ref::<gtk::Widget>(),
            rows.password.upcast_ref(),
            rows.domain.upcast_ref(),
            rows.allow_unencrypted.upcast_ref(),
        ] {
            account.add(row);
        }
        page.add(&account);
        let display = adw::PreferencesGroup::new();
        display.set_title(&i18n("Display"));
        display.add(&rows.scaling);
        display.add(&rows.quality);
        display.add(&rows.vnc_codec);
        display.add(&rows.view_only);
        display.add(&rows.fullscreen);
        display.add(&rows.new_window);
        page.add(&display);
        let network = adw::PreferencesGroup::new();
        network.set_title(&i18n("Network"));
        network.add(&rows.network);
        network.add(&rows.mac);
        network.add(&rows.ssh);
        network.add(&rows.gateway);
        page.add(&network);
        let sharing = adw::PreferencesGroup::new();
        sharing.set_title(&i18n("Sharing"));
        sharing.add(&rows.clipboard);
        sharing.add(&rows.clipboard_files);
        sharing.add(&rows.sound);
        sharing.add(&rows.microphone);
        sharing.add(&rows.folder);
        page.add(&sharing);
        let commands = adw::PreferencesGroup::new();
        commands.set_title(&i18n("Commands"));
        commands.set_description(Some(&i18n(
            "Run by sh, with the connection in $MOTE_HOST, $MOTE_PORT, $MOTE_USER and $MOTE_NAME. A failed command before connecting stops the connection.",
        )));
        commands.add(&rows.command_before);
        commands.add(&rows.command_after);
        page.add(&commands);
        follow_protocol(&rows, c.protocol);

        if !init.is_new {
            for kind in Secret::ALL {
                let id = c.id.clone();
                sender.oneshot_command(async move {
                    EditorCmd::PasswordLoaded(kind, secrets::password(id, kind).await)
                });
            }
        }

        let mut model = Editor {
            valid: false,
            base: init.connection,
            rows,
            edited: Vec::new(),
            filling,
            choice,
            saved: false,
        };
        model.check();
        let widgets = view_output!();
        root.set_default_widget(Some(&widgets.save));
        if init.is_new && !model.base.host.is_empty() {
            model.rows.name.grab_focus();
        } else {
            model.rows.host.grab_focus();
        }
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>, root: &Self::Root) {
        match msg {
            EditorMsg::Changed => self.check(),
            EditorMsg::PasswordEdited(kind) => {
                if !self.edited.contains(&kind) {
                    self.edited.push(kind);
                }
            }
            EditorMsg::OthersOpened if !self.rows.others.is_empty() => {}
            EditorMsg::OthersOpened | EditorMsg::AddAddress => {
                let row = other_row("", &sender);
                self.rows.other.add_row(&row);
                self.rows.others.push(row.clone());
                self.rows.other.set_expanded(true);
                // It can't take the focus until it is shown.
                gtk::glib::idle_add_local_once(move || {
                    row.grab_focus();
                });
            }
            EditorMsg::RemoveAddress(row) => {
                self.rows.other.remove(&row);
                self.rows.others.retain(|other| *other != row);
                if self.rows.others.is_empty() {
                    self.rows.other.set_expanded(false);
                }
                self.check();
            }
            EditorMsg::MoveAddress { row, beside, after } => {
                self.move_address(&row, &beside, after);
            }
            EditorMsg::NudgeAddress { row, down } => {
                let others = &self.rows.others;
                let at = others.iter().position(|other| *other == row);
                let neighbor = at
                    .and_then(|at| {
                        if down {
                            at.checked_add(1)
                        } else {
                            at.checked_sub(1)
                        }
                    })
                    .and_then(|at| others.get(at))
                    .cloned();
                if let Some(beside) = neighbor {
                    self.move_address(&row, &beside, down);
                    // Taking it out of the list took the focus from it. Its
                    // text stays unselected, for a key not to replace it.
                    if let Some(text) = row.delegate().and_downcast::<gtk::Text>() {
                        text.grab_focus_without_selecting();
                    }
                }
            }
            EditorMsg::Cancel => {
                root.close();
            }
            EditorMsg::Delete => {
                let _ = sender.output(EditorOutput::Delete(self.base.id.clone()));
            }
            EditorMsg::Save => {
                if !self.valid {
                    return;
                }
                let r = &self.rows;
                // A port typed but not yet committed by leaving the field.
                r.port.update();
                let connection = Connection {
                    name: r.name.text().into(),
                    protocol: Protocol::ALL
                        .get(r.protocol.selected() as usize)
                        .copied()
                        .unwrap_or_default(),
                    host: r.host.text().into(),
                    port: r.port.value() as u16,
                    other_addresses: r.others.iter().map(|row| row.text().into()).collect(),
                    group: r.group.group(),
                    username: r.username.text().into(),
                    domain: r.domain.text().into(),
                    scaling: Scaling::ALL
                        .get(r.scaling.selected() as usize)
                        .copied()
                        .unwrap_or_default(),
                    quality: Quality::ALL
                        .get(r.quality.selected() as usize)
                        .copied()
                        .unwrap_or_default(),
                    network: Network::ALL
                        .get(r.network.selected() as usize)
                        .copied()
                        .unwrap_or_default(),
                    vnc_codec: VncCodec::ALL
                        .get(r.vnc_codec.selected() as usize)
                        .copied()
                        .unwrap_or_default(),
                    view_only: r.view_only.is_active(),
                    allow_unencrypted: r.allow_unencrypted.is_active(),
                    fullscreen: r.fullscreen.is_active(),
                    new_window: r.new_window.is_active(),
                    command_before: r.command_before.text().into(),
                    command_after: r.command_after.text().into(),
                    clipboard: r.clipboard.is_active(),
                    clipboard_files: r.clipboard_files.is_active(),
                    sound: r.sound.is_active(),
                    microphone: r.microphone.is_active(),
                    folder: r.folder_path.borrow().clone(),
                    ssh_tunnel: r.ssh.enables_expansion(),
                    ssh_host: r.ssh_host.text().into(),
                    ssh_username: r.ssh_username.text().into(),
                    gateway: r.gateway.enables_expansion(),
                    gateway_host: r.gateway_host.text().into(),
                    gateway_username: r.gateway_username.text().into(),
                    mac_address: r.mac.text().into(),
                    robot: self.choice.borrow().robot.clone(),
                    picture: self.choice.borrow().picture.clone(),
                    ..self.base.clone()
                }
                .normalized();
                // Only the image kept stays: the others tried meanwhile go,
                // and so does the one it replaced.
                let choice = self.choice.borrow();
                for name in choice.imported.iter().filter(|n| **n != choice.picture) {
                    avatar::delete_picture(name);
                }
                if self.base.picture != choice.picture {
                    avatar::delete_picture(&self.base.picture);
                }
                drop(choice);
                self.saved = true;
                let passwords = self
                    .edited
                    .iter()
                    .map(|&kind| (kind, Password::new(self.password_row(kind).text().into())))
                    .collect();
                let _ = sender.output(EditorOutput::Saved {
                    connection: Box::new(connection),
                    passwords,
                });
                root.close();
            }
        }
    }

    /// Closed without saving: the images tried meanwhile go.
    fn shutdown(&mut self, _: &mut Self::Widgets, _: relm4::Sender<Self::Output>) {
        if !self.saved {
            for name in &self.choice.borrow().imported {
                avatar::delete_picture(name);
            }
        }
    }

    fn update_cmd(&mut self, msg: Self::CommandOutput, _: ComponentSender<Self>, _: &Self::Root) {
        match msg {
            EditorCmd::PasswordLoaded(kind, Ok(Some(password))) if !self.edited.contains(&kind) => {
                self.filling.set(true);
                self.password_row(kind).set_text(&password);
                self.filling.set(false);
            }
            EditorCmd::PasswordLoaded(_, Err(e)) => {
                tracing::warn!("reading the saved password: {e}");
            }
            EditorCmd::PasswordLoaded(..) => {}
        }
    }
}

impl Editor {
    fn password_row(&self, kind: Secret) -> &adw::PasswordEntryRow {
        match kind {
            Secret::Account => &self.rows.password,
            Secret::Ssh => &self.rows.ssh_password,
            Secret::Gateway => &self.rows.gateway_password,
        }
    }

    fn move_address(&mut self, row: &adw::EntryRow, beside: &adw::EntryRow, after: bool) {
        moved(&mut self.rows.others, row, beside, after);
        // The expander row can only add at the end, so all of them go back
        // on in their new order.
        for other in &self.rows.others {
            self.rows.other.remove(other);
        }
        for other in &self.rows.others {
            self.rows.other.add_row(other);
        }
    }

    /// Whether what the rows hold can be saved, marking the rows that
    /// can't be.
    fn check(&mut self) {
        let r = &self.rows;
        let address = |row: &adw::EntryRow| split_address(&row.text()).is_some();
        let mac = r.mac.text();
        let others = r
            .others
            .iter()
            .map(|row| (row, row.text().trim().is_empty() || address(row)));
        let checks = [
            (&r.host, address(&r.host) || r.host.text().trim().is_empty()),
            (
                &r.ssh_host,
                !r.ssh.enables_expansion() || address(&r.ssh_host),
            ),
            (
                &r.gateway_host,
                !r.gateway.enables_expansion() || address(&r.gateway_host),
            ),
            (&r.mac, mac.trim().is_empty() || parse_mac(&mac).is_some()),
        ];
        let mut valid = address(&r.host);
        for (row, ok) in checks.into_iter().chain(others) {
            // An empty row is still being filled in, not wrong.
            if ok || row.text().trim().is_empty() {
                row.remove_css_class("error");
            } else {
                row.add_css_class("error");
            }
            valid &= ok;
        }
        self.valid = valid;
    }
}

/// Show only what applies to the protocol chosen, and move the port to
/// the new protocol's own while it is still the old one's. VNC takes no
/// domain, none of RDP's codecs or gateway, and shares the clipboard's
/// text alone. Only VNC can go without encryption.
fn follow_protocol(rows: &Rows, protocol: Protocol) {
    let vnc_only = [
        rows.allow_unencrypted.clone().upcast::<gtk::Widget>(),
        rows.vnc_codec.clone().upcast(),
    ];
    let rdp_only: Vec<gtk::Widget> = vec![
        rows.domain.clone().upcast(),
        rows.quality.clone().upcast(),
        rows.network.clone().upcast(),
        rows.gateway.clone().upcast(),
        rows.clipboard_files.clone().upcast(),
        rows.sound.clone().upcast(),
        rows.microphone.clone().upcast(),
        rows.folder.clone().upcast(),
    ];
    let clipboard = rows.clipboard.clone();
    let show = move |protocol: Protocol| {
        clipboard.set_subtitle(&match protocol {
            Protocol::Rdp => i18n("Text only with KRdp; xrdp leaves out folders"),
            Protocol::Vnc => i18n("Text only"),
        });
        for row in &rdp_only {
            row.set_visible(protocol == Protocol::Rdp);
        }
        for row in &vnc_only {
            row.set_visible(protocol == Protocol::Vnc);
        }
    };
    show(protocol);
    let (port, last) = (rows.port.clone(), Cell::new(protocol));
    rows.protocol.connect_selected_notify(move |row| {
        let protocol = Protocol::ALL
            .get(row.selected() as usize)
            .copied()
            .unwrap_or_default();
        if port.value() as u16 == last.get().port() {
            port.set_value(protocol.port().into());
        }
        last.set(protocol);
        show(protocol);
    });
}

fn password_entry(title: &str) -> adw::PasswordEntryRow {
    let row = adw::PasswordEntryRow::new();
    row.set_title(title);
    row.set_activates_default(true);
    row
}

pub(super) fn entry(title: &str, text: &str) -> adw::EntryRow {
    let row = adw::EntryRow::new();
    row.set_title(title);
    row.set_text(text);
    row.set_activates_default(true);
    row
}

pub(crate) fn quality_label(quality: Quality) -> String {
    match quality {
        Quality::Best => i18n("Best Available"),
        Quality::Avc420 => i18n("H.264 (AVC420)"),
        Quality::RemoteFx => i18n("RemoteFX"),
    }
}

/// What choosing it costs, for the row's subtitle.
pub(crate) fn quality_cost(quality: Quality) -> String {
    match quality {
        Quality::Best => String::new(),
        Quality::Avc420 => i18n("Half the color detail: colored text is softer"),
        Quality::RemoteFx => i18n("About twice the bandwidth; KRdp turns it down"),
    }
}

fn vnc_codec_label(codec: VncCodec) -> String {
    match codec {
        VncCodec::Best => i18n("Best Available"),
        VncCodec::Lossless => i18n("Lossless"),
        VncCodec::LowBandwidth => i18n("Low Bandwidth"),
    }
}

/// What choosing it costs, for the row's subtitle, as measured on moving
/// pictures against Best Available.
fn vnc_codec_cost(codec: VncCodec) -> String {
    match codec {
        VncCodec::Best => String::new(),
        VncCodec::Lossless => i18n("About three times the bandwidth"),
        VncCodec::LowBandwidth => i18n("A softer picture, for half the bandwidth"),
    }
}

fn network_label(network: Network) -> String {
    match network {
        Network::Lan => i18n("Local Network"),
        Network::Wan => i18n("Wide Area Network"),
        Network::BroadbandHigh => i18n("Fast Broadband"),
        Network::Satellite => i18n("Satellite"),
        Network::BroadbandLow => i18n("Slow Broadband"),
        Network::Modem => i18n("Modem"),
    }
}

/// The shared folder's row: the folder, a button to choose one and one to
/// stop sharing it.
fn folder_row(row: &adw::ActionRow, path: &Rc<RefCell<String>>) {
    row.set_title(&i18n("Shared Folder"));
    let choose = gtk::Button::builder()
        .icon_name("folder-open-symbolic")
        .tooltip_text(i18n("Choose Folder"))
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    let clear = gtk::Button::builder()
        .icon_name("edit-clear-symbolic")
        .tooltip_text(i18n("Stop Sharing"))
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    choose.update_property(&[gtk::accessible::Property::Label(&i18n("Choose Folder"))]);
    clear.update_property(&[gtk::accessible::Property::Label(&i18n("Stop Sharing"))]);
    row.add_suffix(&clear);
    row.add_suffix(&choose);
    row.set_activatable_widget(Some(&choose));

    let show = {
        let (row, clear) = (row.clone(), clear.clone());
        move |folder: &str| {
            clear.set_visible(!folder.is_empty());
            if folder.is_empty() {
                row.set_subtitle(&i18n("Only xrdp opens a shared folder"));
            } else {
                row.set_subtitle(&gtk::glib::markup_escape_text(&shown_path(folder)));
            }
        }
    };
    show(&path.borrow());
    let show = Rc::new(show);

    clear.connect_clicked({
        let (path, show) = (path.clone(), show.clone());
        move |_| {
            path.borrow_mut().clear();
            show("");
        }
    });
    choose.connect_clicked({
        let (path, show) = (path.clone(), show.clone());
        move |button| {
            let dialog = gtk::FileDialog::builder()
                .title(i18n("Shared Folder"))
                .modal(true)
                .build();
            let current = path.borrow().clone();
            if !current.is_empty() {
                dialog.set_initial_folder(Some(&gtk::gio::File::for_path(&current)));
            }
            let window = button.root().and_downcast::<gtk::Window>();
            let (path, show) = (path.clone(), show.clone());
            dialog.select_folder(
                window.as_ref(),
                None::<&gtk::gio::Cancellable>,
                move |chosen| {
                    let Some(folder) = chosen.ok().and_then(|f| f.path()) else {
                        return;
                    };
                    let folder = folder.to_string_lossy().into_owned();
                    show(&folder);
                    *path.borrow_mut() = folder;
                },
            );
        }
    });
}

/// A path as people read it, with the home folder as ~.
fn shown_path(path: &str) -> String {
    let home = gtk::glib::home_dir();
    match std::path::Path::new(path).strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.to_owned(),
    }
}
