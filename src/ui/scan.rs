//! The Scan Network dialog: the RDP and VNC servers on the local networks, as they
//! answer, each a button away from the editor with its address filled in,
//! and a page of the networks to scan besides this computer's own.

use std::net::SocketAddrV4;
use std::time::Duration;

use adw::prelude::*;
use relm4::prelude::*;
use tokio::sync::oneshot;

use crate::connection::Protocol;
use crate::i18n::{i18n, i18n_f};
use crate::rdp::server::Server;
use crate::scan::{self, Event, Found, Network, NetworkError};

/// Below this, how long a network takes isn't worth saying.
const WORTH_SAYING: Duration = Duration::from_secs(60);

pub(crate) struct Scan {
    /// The host and port of each connection there is.
    saved: Vec<(String, u16)>,
    /// All the scan covers, as the running scan reported it.
    networks: Vec<Network>,
    /// Those the user added, beside their rows in `named_group`.
    named: Vec<(Network, adw::ActionRow)>,
    /// `named` changed since the scan on show started.
    named_changed: bool,
    /// The rows of this computer's own networks.
    local_rows: Vec<adw::ActionRow>,
    /// What answered, in order of address, beside its row in `list`.
    entries: Vec<Entry>,
    list: gtk::ListBox,
    navigation: adw::NavigationView,
    toasts: adw::ToastOverlay,
    entry: adw::EntryRow,
    named_group: adw::PreferencesGroup,
    local_group: adw::PreferencesGroup,
    scanning: bool,
    progress: f64,
    /// Which scan is the current one, as events of the last may still be on
    /// their way.
    run: u64,
    /// Dropped to end the current scan.
    stop: Option<oneshot::Sender<()>>,
}

struct Entry {
    found: Found,
    row: adw::ActionRow,
    add: gtk::Button,
    added: gtk::Label,
}

#[derive(Debug)]
pub(crate) enum ScanMsg {
    Again,
    /// A row's Add button: over to the editor.
    Add(SocketAddrV4),
    /// The editor saved a connection for that row, with this host and port.
    Saved {
        address: SocketAddrV4,
        host: String,
        port: u16,
    },
    ShowNetworks,
    AddNetwork(String),
    RemoveNetwork(Network),
    /// Back from the networks to the computers.
    NetworksClosed,
}

#[derive(Debug)]
pub(crate) enum ScanOutput {
    Add(Box<Found>),
    /// The networks to keep for next time.
    Networks(Vec<String>),
    Closed,
}

pub(crate) struct ScanInit {
    /// The host and port of each connection there is.
    pub(crate) saved: Vec<(String, u16)>,
    /// The networks to scan besides this computer's own.
    pub(crate) named: Vec<Network>,
}

#[relm4::component(pub(crate))]
impl Component for Scan {
    type Init = ScanInit;
    type Input = ScanMsg;
    type Output = ScanOutput;
    type CommandOutput = (u64, Event);

    view! {
        adw::Dialog {
            set_title: &i18n("Scan Network"),
            set_content_width: 460,
            set_content_height: 520,
            connect_closed[sender] => move |_| {
                let _ = sender.output(ScanOutput::Closed);
            },

            #[wrap(Some)]
            #[local_ref]
            set_child = toasts -> adw::ToastOverlay {
                #[wrap(Some)]
                #[local_ref]
                set_child = navigation -> adw::NavigationView {
                    connect_popped[sender] => move |_, _| {
                        sender.input(ScanMsg::NetworksClosed);
                    },

                    add = &adw::NavigationPage {
                        set_title: &i18n("Scan Network"),
                        set_tag: Some("computers"),

                        #[wrap(Some)]
                        set_child = &adw::ToolbarView {
                            add_top_bar = &adw::HeaderBar {
                                pack_start = &gtk::Button {
                                    set_icon_name: "view-refresh-symbolic",
                                    set_tooltip_text: Some(&i18n("Scan Again")),
                                    update_property: &[gtk::accessible::Property::Label(&i18n("Scan Again"))],
                                    #[watch]
                                    set_sensitive: !model.scanning,
                                    connect_clicked => ScanMsg::Again,
                                },
                                pack_end = &gtk::Button {
                                    set_icon_name: "network-wired-symbolic",
                                    set_tooltip_text: Some(&i18n("Networks")),
                                    update_property: &[gtk::accessible::Property::Label(&i18n("Networks"))],
                                    connect_clicked => ScanMsg::ShowNetworks,
                                },
                            },
                            add_top_bar = &gtk::ProgressBar {
                                add_css_class: "osd",
                                #[watch]
                                set_visible: model.scanning,
                                #[watch]
                                set_fraction: model.progress,
                            },

                            #[wrap(Some)]
                            set_content = &gtk::Stack {
                                add_named[Some("scanning")] = &adw::StatusPage {
                                    set_paintable: Some(&adw::SpinnerPaintable::new(Some(&root))),
                                    set_title: &i18n("Scanning…"),
                                    #[watch]
                                    set_description: Some(&model.ranges()),
                                },
                                add_named[Some("none")] = &adw::StatusPage {
                                    set_icon_name: Some("network-wired-symbolic"),
                                    set_title: &i18n("No Computers Found"),
                                    #[watch]
                                    set_description: Some(&i18n_f(
                                        "Nothing on {networks} answered on port {first} or {second}.",
                                        &[
                                            ("networks", &model.ranges()),
                                            ("first", &scan::PORTS[0]),
                                            ("second", &scan::PORTS[1]),
                                        ],
                                    )),

                                    #[wrap(Some)]
                                    set_child = &gtk::Button {
                                        set_label: &i18n("Add Network"),
                                        set_halign: gtk::Align::Center,
                                        add_css_class: "pill",
                                        connect_clicked => ScanMsg::ShowNetworks,
                                    },
                                },
                                add_named[Some("no-network")] = &adw::StatusPage {
                                    set_icon_name: Some("network-offline-symbolic"),
                                    set_title: &i18n("No Local Network"),
                                    set_description: Some(&i18n("This computer has no address on a private network.")),

                                    #[wrap(Some)]
                                    set_child = &gtk::Button {
                                        set_label: &i18n("Add Network"),
                                        set_halign: gtk::Align::Center,
                                        add_css_class: "pill",
                                        connect_clicked => ScanMsg::ShowNetworks,
                                    },
                                },
                                add_named[Some("found")] = &adw::PreferencesPage {
                                    add = &adw::PreferencesGroup {
                                        set_title: &i18n("Computers"),
                                        #[watch]
                                        set_description: Some(&model.ranges()),
                                        #[local_ref]
                                        add = list -> gtk::ListBox {
                                            add_css_class: "boxed-list",
                                            set_selection_mode: gtk::SelectionMode::None,
                                        },
                                    },
                                },
                                // After the pages: naming one before it exists only
                                // earns a warning.
                                #[watch]
                                set_visible_child_name: model.page(),
                            },
                        },
                    },

                    add = &adw::NavigationPage {
                        set_title: &i18n("Networks"),
                        set_tag: Some("networks"),

                        #[wrap(Some)]
                        set_child = &adw::ToolbarView {
                            add_top_bar = &adw::HeaderBar {},

                            #[wrap(Some)]
                            set_content = &adw::PreferencesPage {
                                #[local_ref]
                                add = local_group -> adw::PreferencesGroup {
                                    set_title: &i18n("This Computer"),
                                    #[watch]
                                    set_visible: !model.local_rows.is_empty(),
                                },
                                #[local_ref]
                                add = named_group -> adw::PreferencesGroup {
                                    set_title: &i18n("Scan additional subnets"),
                                    #[local_ref]
                                    add = entry -> adw::EntryRow {
                                        set_title: &i18n("Network, Such As 192.168.2.0/24"),
                                        set_show_apply_button: true,
                                        connect_apply[sender] => move |row| {
                                            sender.input(ScanMsg::AddNetwork(row.text().to_string()));
                                        },
                                        connect_changed => move |row| {
                                            row.remove_css_class("error");
                                        },
                                    },
                                },
                            },
                        },
                    },
                },
            },
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let mut model = Scan {
            saved: init.saved,
            networks: Vec::new(),
            named: Vec::new(),
            named_changed: false,
            local_rows: Vec::new(),
            entries: Vec::new(),
            list: gtk::ListBox::new(),
            navigation: adw::NavigationView::new(),
            toasts: adw::ToastOverlay::new(),
            entry: adw::EntryRow::new(),
            named_group: adw::PreferencesGroup::new(),
            local_group: adw::PreferencesGroup::new(),
            scanning: false,
            progress: 0.0,
            run: 0,
            stop: None,
        };
        let list = &model.list.clone();
        let navigation = &model.navigation.clone();
        let toasts = &model.toasts.clone();
        let entry = &model.entry.clone();
        let named_group = &model.named_group.clone();
        let local_group = &model.local_group.clone();
        let widgets = view_output!();
        for network in init.named {
            model.show_named(network, &sender);
        }
        model.start(&sender);
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>, _: &Self::Root) {
        match msg {
            ScanMsg::Again => self.start(&sender),
            ScanMsg::Add(address) => {
                if let Some(entry) = self.entries.iter().find(|e| e.found.address == address) {
                    let _ = sender.output(ScanOutput::Add(Box::new(entry.found.clone())));
                }
            }
            ScanMsg::Saved {
                address,
                host,
                port,
            } => {
                // As saved, which the editor may have changed, for the next
                // scan to compare with.
                self.saved.push((host, port));
                if let Some(entry) = self.entries.iter_mut().find(|e| e.found.address == address) {
                    entry.found.saved = true;
                    entry.show_added();
                }
            }
            ScanMsg::ShowNetworks => {
                self.navigation.push_by_tag("networks");
                self.entry.grab_focus();
            }
            ScanMsg::AddNetwork(text) => self.add_network(text.trim(), &sender),
            ScanMsg::RemoveNetwork(network) => {
                if let Some(at) = self.named.iter().position(|(n, _)| *n == network) {
                    let (_, row) = self.named.remove(at);
                    self.named_group.remove(&row);
                    self.networks.retain(|n| *n != network);
                    self.save_named(&sender);
                }
            }
            ScanMsg::NetworksClosed => {
                if self.named_changed {
                    self.start(&sender);
                }
            }
        }
    }

    fn update_cmd(
        &mut self,
        (run, event): Self::CommandOutput,
        sender: ComponentSender<Self>,
        _: &Self::Root,
    ) {
        if run != self.run {
            return;
        }
        match event {
            Event::Networks(networks) => {
                self.scanning = !networks.is_empty();
                self.show_local(&networks);
                self.networks = networks;
            }
            Event::Progress(progress) => self.progress = progress,
            Event::Found(found) => self.show(*found, &sender),
            Event::Done => self.scanning = false,
        }
    }
}

impl Scan {
    /// Start over, ending any scan still under way, whose networks may no
    /// longer be the ones wanted.
    fn start(&mut self, sender: &ComponentSender<Self>) {
        for entry in self.entries.drain(..) {
            self.list.remove(&entry.row);
        }
        self.named_changed = false;
        self.scanning = true;
        self.progress = 0.0;
        self.run += 1;
        let (stop, stopped) = oneshot::channel::<()>();
        self.stop = Some(stop);
        let run = self.run;
        let saved = self.saved.clone();
        let named = self.named.iter().map(|(network, _)| *network).collect();
        sender.command(move |out, shutdown| {
            shutdown
                .register(async move {
                    let scan = scan::run(saved, named, |event| {
                        let _ = out.send((run, event));
                    });
                    tokio::select! {
                        () = scan => {}
                        _ = stopped => {}
                    }
                })
                .drop_on_shutdown()
        });
    }

    fn add_network(&mut self, text: &str, sender: &ComponentSender<Self>) {
        let message = match text.parse::<Network>() {
            Ok(network)
                if self.networks.contains(&network)
                    || self.named.iter().any(|(n, _)| *n == network) =>
            {
                i18n_f("{network} is already scanned", &[("network", &network)])
            }
            Ok(network) => {
                self.show_named(network, sender);
                self.save_named(sender);
                self.entry.set_text("");
                return;
            }
            Err(NetworkError::NotANetwork) => i18n("Not a network, such as 192.168.2.0/24"),
            Err(NetworkError::NotPrivate) => i18n("Only private networks can be scanned"),
            Err(NetworkError::TooWide) => i18n("A network can be /16 at the widest"),
        };
        self.entry.add_css_class("error");
        self.toasts.add_toast(adw::Toast::new(&message));
    }

    fn save_named(&mut self, sender: &ComponentSender<Self>) {
        self.named_changed = true;
        let named = self.named.iter().map(|(n, _)| n.to_string()).collect();
        let _ = sender.output(ScanOutput::Networks(named));
    }

    /// A row of `network`, which the user can take out again.
    fn show_named(&mut self, network: Network, sender: &ComponentSender<Self>) {
        let name = network.to_string();
        let row = adw::ActionRow::new();
        row.set_title(&name);
        let longest = scan::longest_scan(network);
        if longest >= WORTH_SAYING {
            row.set_subtitle(&i18n_f(
                "Up to {minutes} min to scan",
                &[("minutes", &longest.as_secs().div_ceil(60))],
            ));
        }
        let remove = gtk::Button::from_icon_name("user-trash-symbolic");
        remove.add_css_class("flat");
        remove.set_valign(gtk::Align::Center);
        remove.set_tooltip_text(Some(&i18n("Remove")));
        remove.update_property(&[gtk::accessible::Property::Label(&i18n_f(
            "Remove {name}",
            &[("name", &name)],
        ))]);
        let input = sender.input_sender().clone();
        remove.connect_clicked(move |_| input.emit(ScanMsg::RemoveNetwork(network)));
        row.add_suffix(&remove);
        self.named_group.add(&row);
        self.named.push((network, row));
    }

    /// List the networks a scan covers that the user didn't add.
    fn show_local(&mut self, networks: &[Network]) {
        for row in self.local_rows.drain(..) {
            self.local_group.remove(&row);
        }
        for network in networks {
            if !self.named.iter().any(|(n, _)| n == network) {
                let row = adw::ActionRow::new();
                row.set_title(&network.to_string());
                self.local_group.add(&row);
                self.local_rows.push(row);
            }
        }
    }

    fn page(&self) -> &'static str {
        if !self.entries.is_empty() {
            "found"
        } else if self.scanning {
            "scanning"
        } else if self.networks.is_empty() {
            "no-network"
        } else {
            "none"
        }
    }

    /// The ranges being scanned, for the page to say how far it looks.
    fn ranges(&self) -> String {
        let ranges: Vec<String> = self.networks.iter().map(Network::to_string).collect();
        ranges.join(", ")
    }

    /// Add a row in its place by address, so that the list doesn't depend
    /// on who answers first.
    fn show(&mut self, found: Found, sender: &ComponentSender<Self>) {
        let at = self
            .entries
            .partition_point(|e| e.found.address < found.address);
        let entry = Entry::new(found, sender);
        self.list.insert(&entry.row, at as i32);
        self.entries.insert(at, entry);
    }
}

impl Entry {
    fn new(found: Found, sender: &ComponentSender<Scan>) -> Self {
        let title = title(&found);
        let row = adw::ActionRow::new();
        // Names from the network, so they aren't to be taken for markup.
        row.set_use_markup(false);
        row.set_title(&title);
        row.set_subtitle(&subtitle(&found));

        let add = gtk::Button::with_label(&i18n("Add"));
        add.set_valign(gtk::Align::Center);
        add.update_property(&[gtk::accessible::Property::Label(&i18n_f(
            "Add {name}",
            &[("name", &title)],
        ))]);
        let (input, address) = (sender.input_sender().clone(), found.address);
        add.connect_clicked(move |_| input.emit(ScanMsg::Add(address)));
        row.add_suffix(&add);

        let added = gtk::Label::new(Some(&i18n("Added")));
        added.add_css_class("dim-label");
        row.add_suffix(&added);

        let entry = Self {
            found,
            row,
            add,
            added,
        };
        if entry.found.saved {
            entry.show_added();
        } else {
            entry.added.set_visible(false);
            entry.row.set_activatable_widget(Some(&entry.add));
        }
        entry
    }

    fn show_added(&self) {
        let had_focus = self.add.has_focus();
        self.add.set_visible(false);
        self.added.set_visible(true);
        self.row.set_activatable_widget(gtk::Widget::NONE);
        self.row.set_activatable(false);
        // The button is gone; the keyboard stays where it was in the list.
        if had_focus {
            self.row.grab_focus();
        }
    }
}

fn title(found: &Found) -> String {
    if found.name.is_empty() {
        found.host.clone()
    } else {
        found.name.clone()
    }
}

/// The address, unless the title is that already; VNC, which is less
/// usual; and the server, if its certificate says which.
fn subtitle(found: &Found) -> String {
    let address = found.address;
    let at = if address.port() == found.protocol.port() {
        address.ip().to_string()
    } else {
        address.to_string()
    };
    let parts = [
        (title(found) != at).then_some(at),
        (found.protocol == Protocol::Vnc).then(|| Protocol::Vnc.name().to_owned()),
        found.server.map(server_name),
    ];
    let parts: Vec<String> = parts.into_iter().flatten().collect();
    parts.join(" · ")
}

/// A certificate tells GNOME's server from the others, but not its Desktop
/// Sharing from its Remote Login: both make it the same way.
fn server_name(server: Server) -> String {
    match server {
        Server::GnomeDesktop | Server::GnomeLogin => i18n("GNOME Remote Desktop"),
        Server::Krdp => "KRdp".to_owned(),
        Server::Xrdp => "xrdp".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(address: &str, host: &str, name: &str, server: Option<Server>) -> Found {
        Found {
            address: address.parse().unwrap(),
            host: host.into(),
            name: name.into(),
            server,
            protocol: crate::scan::protocol(address.parse::<SocketAddrV4>().unwrap().port()),
            saved: false,
        }
    }

    #[test]
    fn rows_say_the_address_once_and_the_port_when_unusual() {
        let by_name = found("10.0.1.33:3389", "vm.lan", "", Some(Server::Xrdp));
        assert_eq!(title(&by_name), "vm.lan");
        assert_eq!(subtitle(&by_name), "10.0.1.33 · xrdp");

        let bare = found("10.0.1.33:3389", "10.0.1.33", "", None);
        assert_eq!(title(&bare), "10.0.1.33");
        assert_eq!(subtitle(&bare), "");

        let other_port = found("10.0.1.33:3390", "10.0.1.33", "", Some(Server::Krdp));
        assert_eq!(subtitle(&other_port), "10.0.1.33:3390 · KRdp");

        let named = found("10.0.1.33:3389", "10.0.1.33", "build01", None);
        assert_eq!(title(&named), "build01");
        assert_eq!(subtitle(&named), "10.0.1.33");

        let vnc = found("10.0.1.17:5900", "mint.lan", "", None);
        assert_eq!(subtitle(&vnc), "10.0.1.17 · VNC");
        let display = found("10.0.1.17:5901", "10.0.1.17", "", None);
        assert_eq!(subtitle(&display), "10.0.1.17:5901 · VNC");
    }
}
