<p align="center">
  <img src="data/repo/logo.png" width="120" alt="Mote logo">
</p>

# Mote

A GNOME-native RDP and VNC client specifically designed for connecting to Linux desktops. Full support for GNOME Remote Desktop, xrdp, KRdp, TigerVNC and wayvnc, and for Macs through macOS Screen Sharing. Available for Linux and macOS

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="data/repo/screenshot-dark.png">
    <img src="data/repo/screenshot.png" width="900" alt="Mote showing a session beside the saved connections">
  </picture>
</p>

## Goals

- Linux machines first: Mote is built and tested against GNOME Remote Desktop (Desktop Sharing and Remote
  Login), xrdp and KRdp over RDP, and GNOME Remote Desktop, TigerVNC and
  wayvnc over VNC. 
- Mac compatibility: Tested against macOS Screen Sharing over VNC signed in and encrypted.
  Will automatically choose the correct, most compatible settings based on the macOS version.
- Match Remmina's RDP and VNC plugins in a GNOME-native interface.
- Sharp on HiDPI displays: the remote desktop follows the local scale factor including fractional scaling. 
  Automatic resizing of the remote resolution with a compatible session.
- Clipboard sharing in both directions: text, rich text, images and files
  over RDP, and text over VNC.
- Encrypted by default: a VNC server that offers no encryption stops the
  connection before the password goes out. This can be overridden on a per-connection basis.
- Fast: RDP H.264 decoding through FFmpeg or OpenH264.
  VNC encoding through wayvnc's Open H.264. No work is performed on the main thread that the
  display does not need.
- A small memory footprint targeted below 500MB. Expect memory footprints around 350MB on the RPM/DEB package.
- Settings per connection with defaults that suit a Linux RDP/VNC server.

## Installing

```sh
flatpak install --user --from https://mote.hyprlab.co/flatpak/co.hyprlab.Mote.flatpakref
```

Fedora and Debian packages, AppImages, the macOS app and the beta channel are
in [docs/INSTALLING.md](docs/INSTALLING.md).

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

The app icon was made by Jason M (hyprlab) in Blender.

Bug reports and pull requests are welcome whether or not AI tools were
involved, and everything merged is reviewed by a person.

## License

Mote is free software under the
[GNU Affero General Public License v3.0 or later](LICENSE).
