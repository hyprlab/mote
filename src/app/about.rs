//! The About window and the people it thanks, laid out as Hylki's is: the
//! app's name and icon, a version chip and a line on what it is, the
//! release notes a page away, the project's links, and the thanks lists.

mod notes;

use adw::prelude::*;

use crate::config;
use crate::i18n::i18n;
use crate::{APP_ID, APP_NAME, VERSION};

/// The contributors and translators shown in the thanks lists: one person
/// per line, `Display Name <github-handle>`, a translator's languages after
/// a dash. build.rs copies them in from `data/`, or nothing while a file
/// doesn't exist yet. What each person contributed is credited in the
/// changelog, not in the files.
const CONTRIBUTORS: &str = include_str!(concat!(env!("OUT_DIR"), "/CONTRIBUTORS"));
const TRANSLATORS: &str = include_str!(concat!(env!("OUT_DIR"), "/TRANSLATORS"));

const WEBSITE: &str = "https://mote.hyprlab.co";
const REPOSITORY: &str = "https://github.com/hyprlab/mote";

/// One of those files as (display name, GitHub handle, note) rows. Comments,
/// blank lines and any line without a handle are skipped, so a typo in the
/// file costs that one row rather than the list.
fn credits(file: &'static str) -> Vec<(&'static str, &'static str, &'static str)> {
    file.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let (name, rest) = line.split_once('<')?;
            let (handle, note) = rest.split_once('>')?;
            Some((
                name.trim(),
                handle.trim(),
                note.trim().trim_start_matches('-').trim(),
            ))
        })
        .collect()
}

/// Open `uri` in the browser, or the mail app, through the portal under
/// Flatpak.
pub(super) fn open_link(uri: &str, from: &impl IsA<gtk::Widget>) {
    let window = from.root().and_downcast::<gtk::Window>();
    gtk::UriLauncher::new(uri).launch(window.as_ref(), gtk::gio::Cancellable::NONE, |done| {
        if let Err(e) = done {
            tracing::warn!("opening a link: {e}");
        }
    });
}

/// A heading over one of the About page's lists.
fn section_heading(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("heading");
    label.set_halign(gtk::Align::Start);
    label.set_margin_top(20);
    label.set_margin_bottom(6);
    label
}

fn boxed_list() -> gtk::ListBox {
    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::None);
    list
}

/// A row that opens `url`, which its tooltip shows.
fn link_row(title: &str, subtitle: Option<&str>, url: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .activatable(true)
        .build();
    if let Some(subtitle) = subtitle {
        row.set_subtitle(subtitle);
    }
    row.set_tooltip_text(Some(url));
    row.add_suffix(&gtk::Image::from_icon_name("adw-external-link-symbolic"));
    let url = url.to_owned();
    row.connect_activated(move |row| open_link(&url, row));
    row
}

/// The icon and name, the version chip and the one-line description.
fn identity(page: &gtk::Box) {
    let icon = gtk::Image::from_icon_name(APP_ID);
    icon.set_pixel_size(128);
    icon.add_css_class("icon-dropshadow");
    page.append(&icon);

    let name = gtk::Label::new(Some(APP_NAME));
    name.add_css_class("title-1");
    name.set_margin_top(12);
    page.append(&name);

    let version = gtk::Label::new(Some(VERSION));
    version.add_css_class("about-version-chip");
    version.set_halign(gtk::Align::Center);
    version.set_margin_top(8);
    page.append(&version);

    let description = gtk::Label::new(Some(&i18n("GNOME-native RDP and VNC client")));
    description.set_wrap(true);
    description.set_justify(gtk::Justification::Center);
    description.add_css_class("dim-label");
    description.set_margin_top(12);
    page.append(&description);

    if cfg!(feature = "beta") {
        let warning = gtk::Label::new(Some(&i18n(
            "A beta build for trying changes early. Expect bugs, and report them on GitHub. It shares connections and settings with the stable install.",
        )));
        warning.set_wrap(true);
        warning.set_justify(gtk::Justification::Center);
        warning.add_css_class("warning");
        warning.set_margin_top(10);
        page.append(&warning);
    }
}

/// Release Notes, a page of the window.
fn info_rows(page: &gtk::Box, nav: &adw::NavigationView) {
    let info = boxed_list();
    info.set_margin_top(20);
    let notes = adw::ActionRow::builder()
        .title(i18n("Release Notes"))
        .subtitle(crate::i18n::i18n_f(
            "What’s new in {version}",
            &[("version", &VERSION)],
        ))
        .activatable(true)
        .build();
    notes.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    let nav = nav.clone();
    notes.connect_activated(move |_| nav.push_by_tag("notes"));
    info.append(&notes);
    page.append(&info);
}

/// The project's links.
fn links(page: &gtk::Box) {
    page.append(&section_heading(&i18n("Project")));
    let links = boxed_list();
    links.append(&link_row(&i18n("Website"), None, WEBSITE));
    links.append(&link_row(
        &i18n("Report an issue or feature request"),
        None,
        &format!("{REPOSITORY}/issues"),
    ));
    links.append(&link_row(
        "hyprlab@proton.me",
        None,
        "mailto:hyprlab@proton.me",
    ));
    links.append(&link_row(&i18n("Source Code"), None, REPOSITORY));
    links.append(&link_row(
        &i18n("License (GNU AGPL v3)"),
        None,
        "https://www.gnu.org/licenses/agpl-3.0.html",
    ));
    // With a coffee cup for its icon.
    let coffee = link_row(
        &i18n("Buy Me a Coffee"),
        None,
        "https://buymeacoffee.com/hyprlab",
    );
    let cup = gtk::Label::new(Some("☕"));
    cup.add_css_class("about-coffee");
    coffee.add_prefix(&cup);
    links.append(&coffee);
    page.append(&links);
}

/// The people thanked, the artwork used, and the footer.
fn thanks(page: &gtk::Box) {
    let people = |rows: Vec<(&'static str, &'static str, &'static str)>| {
        let list = boxed_list();
        for (name, handle, note) in rows {
            let subtitle = if note.is_empty() {
                format!("@{handle}")
            } else {
                format!("{note} · @{handle}")
            };
            list.append(&link_row(
                name,
                Some(&subtitle),
                &format!("https://github.com/{handle}"),
            ));
        }
        list
    };
    // People outside Hyprlab whose patches are in the app, then those who
    // put it in other languages; each list once it has somebody.
    for (heading, file) in [
        (i18n("Special thanks to these contributors"), CONTRIBUTORS),
        (i18n("Translated by"), TRANSLATORS),
    ] {
        let rows = credits(file);
        if !rows.is_empty() {
            page.append(&section_heading(&heading));
            page.append(&people(rows));
        }
    }

    // The RDP engine, the SSH tunnel, and the robots connections are shown
    // as. CC0 asks for no credit; it is given anyway.
    page.append(&section_heading(&i18n("Built using")));
    let projects = boxed_list();
    projects.append(&link_row(
        "IronRDP",
        Some("MIT, Apache 2.0"),
        "https://github.com/Devolutions/IronRDP",
    ));
    projects.append(&link_row(
        "russh",
        Some("Apache 2.0"),
        "https://github.com/warp-tech/russh",
    ));
    projects.append(&link_row(
        &i18n("Pixelbot by DiceBear"),
        Some("CC0 1.0"),
        "https://www.dicebear.com",
    ));
    page.append(&projects);

    let footer = gtk::Label::new(Some(&i18n("© 2026 Hyprlab")));
    footer.add_css_class("dim-label");
    footer.add_css_class("caption");
    footer.set_wrap(true);
    footer.set_justify(gtk::Justification::Center);
    footer.set_margin_top(20);
    page.append(&footer);
}

/// A scrollable About window: the app up top, then its links and thanks,
/// with Release Notes sliding in over it.
pub(super) fn present(parent: &adw::ApplicationWindow) {
    let title = crate::i18n::i18n_f("About {name}", &[("name", &APP_NAME)]);
    let window = adw::Window::builder()
        .transient_for(parent)
        .modal(false)
        .title(title.as_str())
        .default_width(460)
        // Its height stays as it was left.
        .default_height(config::load_settings().about_height)
        .build();
    // Added, not set: the builder's list would replace `background`.
    window.add_css_class("about-window");
    window.connect_close_request(|w| {
        let mut settings = config::load_settings();
        settings.about_height = w.height();
        config::save_settings(&settings);
        gtk::glib::Propagation::Proceed
    });

    let nav = adw::NavigationView::new();
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .build();
    let clamp = adw::Clamp::builder().maximum_size(420).build();
    let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    page.set_margin_top(18);
    page.set_margin_bottom(12);
    page.set_margin_start(12);
    page.set_margin_end(12);

    identity(&page);
    info_rows(&page, &nav);
    links(&page);
    thanks(&page);

    clamp.set_child(Some(&page));
    scroller.set_child(Some(&clamp));

    let main = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    header.add_css_class("flat");
    header.set_show_title(false);
    main.add_top_bar(&header);
    main.set_content(Some(&scroller));
    nav.add(
        &adw::NavigationPage::builder()
            .title(title.as_str())
            .tag("main")
            .child(&main)
            .build(),
    );
    nav.add(&notes::page());

    window.set_content(Some(&nav));
    window.present();

    // The showcase hook: MOTE_SHOWCASE_SCROLL runs the page down by that
    // fraction once it is laid out, so a capture can show its foot, and
    // MOTE_SHOWCASE_NOTES opens Release Notes.
    if let Some(fraction) = std::env::var("MOTE_SHOWCASE_SCROLL")
        .ok()
        .map(|v| v.parse::<f64>().unwrap_or(1.0))
    {
        let adjustment = scroller.vadjustment();
        gtk::glib::timeout_add_seconds_local_once(2, move || {
            adjustment.set_value(
                adjustment.lower() + (adjustment.upper() - adjustment.page_size()) * fraction,
            );
        });
    }
    if std::env::var_os("MOTE_SHOWCASE_NOTES").is_some() {
        nav.push_by_tag("notes");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The credits files are read at build time and shown in About, so a
    /// line that stops parsing loses a real person from the list silently.
    #[test]
    fn credits_files_parse() {
        for file in [CONTRIBUTORS, TRANSLATORS] {
            let rows = credits(file);
            let lines = file
                .lines()
                .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
                .count();
            assert_eq!(rows.len(), lines, "every non-comment line must parse");
            for (name, handle, _) in rows {
                assert!(!name.is_empty() && !handle.is_empty(), "{name}/{handle}");
                assert!(
                    !handle.contains(char::is_whitespace),
                    "{handle} is not a GitHub handle"
                );
            }
        }
    }

    #[test]
    fn credits_read_names_handles_and_languages() {
        let file = "# a comment\n\nAda Lovelace <ada>\nJo <jo-1> - French, German\n";
        assert_eq!(
            credits(file),
            [
                ("Ada Lovelace", "ada", ""),
                ("Jo", "jo-1", "French, German")
            ]
        );
    }
}
