<p align="center">
  <img src="data/repo/logo.png" width="120" alt="Mote logo">
</p>

# Mote

A GNOME-native RDP and VNC client written in Rust, with an IronRDP backend for RDP and its own for VNC, specifically designed for connecting to Linux desktops. Full support for GNOME Remote Desktop, xrdp, KRdp, TigerVNC and wayvnc.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="data/repo/screenshot-dark.png">
    <img src="data/repo/screenshot.png" width="900" alt="Mote showing a session beside the saved connections">
  </picture>
</p>

Mote 1.0 is the first stable release. It was built in sections, each one
finished and tested against a real server before the next started; what each
section holds is in [docs/ROADMAP.md](docs/ROADMAP.md).

## Goals

- Linux machines first: GNOME Remote Desktop (Desktop Sharing and Remote
  Login), xrdp and KRdp are what Mote is built and tested against. Mote has
  not been tested against a Windows machine.
- Everything Remmina's RDP plugin does, in a GNOME-native interface.
- Sharp on HiDPI displays: the remote desktop follows the local scale factor,
  fractional scaling included, and resizes with the window.
- Clipboard sharing in both directions: text, rich text, images and files.
- Fast: hardware-friendly decoding of the RDP graphics pipeline, and no work
  on the main thread that the display does not need.
- A small memory footprint, so a session costs little beside the rest of
  the desktop.
- Settings per connection, with defaults that suit a Linux server.

## Installing

```sh
flatpak install --user --from https://mote.hyprlab.co/flatpak/co.hyprlab.Mote.flatpakref
```

Fedora and Debian packages, AppImages and the beta channel are in
[docs/INSTALLING.md](docs/INSTALLING.md).

## Building

See [docs/BUILDING.md](docs/BUILDING.md). In short, with GTK 4 and
libadwaita development files installed:

```sh
cargo build
./target/debug/mote
```

## Documentation

Everything else is in [docs/](docs/README.md). To help, see
[docs/CONTRIBUTING.md](docs/CONTRIBUTING.md).

## AI notice

Mote is built by a human maintainer who uses generative AI as a development
tool. The maintainer decides what gets built, reviews the results, tests every
release and signs off on everything that ships. The app itself contains no AI
and makes no requests to AI services.

The app icon was made by Jason M.

Bug reports and pull requests are welcome whether or not AI tools were
involved, and everything merged is reviewed by a person.

## License

Mote is free software under the
[GNU Affero General Public License v3.0 or later](LICENSE).
