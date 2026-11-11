# Building Mote

## Dependencies

- Rust 1.94 or later, through [rustup](https://rustup.rs) or the
  distribution.
- GTK 4.18 or later and libadwaita 1.7 or later, with their development
  files.
- gettext.

On Fedora:

```sh
sudo dnf install gcc gtk4-devel libadwaita-devel gettext-devel
```

On Debian and Ubuntu:

```sh
sudo apt install build-essential libgtk-4-dev libadwaita-1-dev gettext
```

### H.264

Mote decodes H.264 with a library it loads at run time rather than building
in, so that the patent licenses stay with whoever provides the library.
Settings, under Video, chooses one; otherwise, or when the chosen one is
missing, it takes the first it finds:

1. FFmpeg's libavcodec, version 5 to 8. The Flatpak's GNOME runtime has it
   in its codecs-extra extension.
2. Cisco's OpenH264 2.6 from the system (`libopenh264.so.8`).
3. Cisco's OpenH264 downloaded from Cisco, from Settings, under Video. The
   download is checked against Cisco's published hash before it is kept.

Without any of them, servers send RemoteFX instead, which blurs small text,
and KRdp, which sends only H.264, refuses the session.

On Fedora, the system's OpenH264 comes from Fedora's Cisco repository;
`noopenh264`, a stand-in that decodes nothing, is installed by default:

```sh
sudo dnf swap noopenh264 openh264
```

On Debian and Ubuntu:

```sh
sudo apt install libopenh264-8
```

`MOTE_H264` set to `ffmpeg`, `openh264` or `cisco` tries that source alone,
`MOTE_FFMPEG_DIR` names a directory to load FFmpeg from, and `MOTE_OPENH264`
another OpenH264 library.

### Sound

Sound and the microphone go through PulseAudio's simple API
(`libpulse-simple.so.0`), which PipeWire serves, and Opus is decoded with
`libopus.so.0`; both are loaded at run time, and every desktop distribution
installs them. Without libopus, servers send uncompressed PCM, about twelve
times the bandwidth; without libpulse-simple there is no sound.

### VNC encryption

GNOME Remote Desktop's VNC and TigerVNC's VeNCrypt TLS subtypes encrypt
with anonymous TLS, which rustls doesn't implement. For those alone Mote
loads OpenSSL 3 (`libssl.so.3` and `libcrypto.so.3`) at run time, which the
GNOME runtime and every desktop distribution have; nothing links it, so
there is no build dependency. Without it, those servers end the attempt
with a message naming OpenSSL, and every other kind of VNC security still
works.

## Building and running

```sh
cargo build
./target/debug/mote
```

`MOTE_LOG` takes a [tracing filter](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html),
for example `MOTE_LOG=mote=debug`.

A release build (`cargo build --release`) is slower to make and is only
needed for packaging or for judging performance.

GNOME Shell keeps an app's answers, such as letting a session hold the
desktop's shortcuts back, only for an app it knows by its desktop file;
for any other window it asks again every time. To run from the tree,
install one that starts the build:

```sh
sed "s|^Exec=mote$|Exec=$PWD/target/debug/mote|" data/co.hyprlab.Mote.desktop \
  > ~/.local/share/applications/co.hyprlab.Mote.desktop
```

## macOS

On a Mac, with [Homebrew](https://brew.sh):

```sh
brew install gtk4 libadwaita pkgconf adwaita-icon-theme rustup
rustup default stable
tools/build-macos-app.sh
open target/debug/Mote.app
```

`tools/build-macos-app.sh` builds the app and wraps it in `Mote.app`, which
gives it its name in the menu bar and the Dock, and its icon;
`tools/build-macos-app.sh release` does the same with a release build. The
bundle still loads GTK and libadwaita from Homebrew, and it is signed ad hoc,
so the Keychain asks again before each new build reads a saved password.

The Keychain is locked to an SSH login. Over SSH, `open` starts the app in
the session of whoever is logged in at the Mac, where it is unlocked, and
`open --env NAME=value` passes it the environment, such as `MOTE_SHOWCASE`
(see [Pictures of the interface](#pictures-of-the-interface)).

## Translations

`tools/update-pot.sh` regenerates `po/mote.pot` after strings change, with
[xtr](https://crates.io/crates/xtr) (`cargo install xtr`) and gettext's
tools, and brings every `po/<lang>.po` up to date with it.
`tools/build-locale.sh` compiles them so a source-tree run picks them up.
See [po/README.md](../po/README.md).

## Flatpak

The manifest is `co.hyprlab.Mote.yml`, for `flatpak-builder`, on the GNOME
50 runtime. Its crates come from `cargo-sources.json`, which is generated
from `Cargo.lock` with `flatpak-cargo-generator.py` from
[flatpak-builder-tools](https://github.com/flatpak/flatpak-builder-tools)
whenever the lock file changes, and isn't kept in the repository:

```sh
python3 flatpak-cargo-generator.py Cargo.lock -o cargo-sources.json
flatpak install flathub org.gnome.Sdk//50 org.gnome.Platform//50 \
    org.freedesktop.Sdk.Extension.rust-stable//25.08
flatpak-builder --force-clean build-dir co.hyprlab.Mote.yml
flatpak-builder --run build-dir co.hyprlab.Mote.yml mote
```

The last line runs the build without installing it; `--user --install`
installs it. H.264 comes from the runtime's codecs-extra extension, which
Flatpak installs with the runtime from Flathub; elsewhere it is
`org.freedesktop.Platform.codecs-extra//25.08-extra`.

## RPM and Debian packages

`tools/build-packages.sh rpm` and `tools/build-packages.sh deb` build
`packaging/out/mote-<version>-1.<dist>.<arch>.rpm` and
`packaging/out/mote_<version>-1_<arch>.deb`; with no argument it builds
both. Each wraps a release binary with the launcher, metainfo, icons and
translations. The RPM's is built on this machine, for the Fedora release it
runs, and needs `rpm-build` and gettext. The Debian package's is built in
Debian 13 under podman (on a Debian host, in place), the oldest release
with the libadwaita 1.7 Mote needs, so it installs there and on Ubuntu
25.04 and later. The libraries Mote opens itself (PulseAudio's simple API,
Opus, FFmpeg, OpenH264) are recommended by both, as neither format can see
them linked.

Both packages also install Mote's repository on mote.hyprlab.co, so an
install from a download keeps updating with the rest of the system:
`packaging/fedora/mote.repo` and `packaging/debian/mote.sources`, and a beta
repository that is off until turned on. They are the files the site serves
at `/rpm/mote.repo` and `/deb/mote.sources`, byte for byte, the beta ones
but for the line that turns them off, so a repository added by hand and the
package's don't conflict. When the site's signing key changes, they are
written again from what it serves:

```sh
curl -fsS https://mote.hyprlab.co/rpm/mote.repo | diff - packaging/fedora/mote.repo
curl -fsS https://mote.hyprlab.co/deb/mote.sources | diff - packaging/debian/mote.sources
```

## AppImage

`tools/build-appimage.sh` builds `packaging/out/Mote-<arch>.AppImage` and
its `.zsync` in an Arch container under podman, with pkgforge's
[quick-sharun](https://github.com/pkgforge-dev/Anylinux-AppImages), which
bundles every library down to glibc so one file runs on any distribution.
FFmpeg is left out, for H.264 comes from whoever holds its patent license:
the AppImage uses the system's FFmpeg where there is one, and Cisco's
OpenH264 from Settings otherwise. It updates from the GitHub releases, through
the `.zsync` beside each bundle.

## ARM64, and releases

`.github/workflows/build-packages.yml` builds every package above, the
Flatpak as a single-file bundle, for x86_64 and for ARM64 on GitHub's arm64
runners, then installs the RPM and the Debian package on clean Fedora 44
and Debian 13 to check that what they need resolves. It runs on a `v*` tag,
or by hand for any ref. A tag also gathers every package into a draft
release, to be read over and published by hand, with the notes
[RELEASING.md](RELEASING.md) describes.

A version `X.Y.Z-beta.N` is a beta, and its tag's draft is a prerelease. Its
Flatpak and AppImage are the beta app, `Mote-Beta-<arch>`: built with the
`beta` cargo feature, app ID `co.hyprlab.Mote.Beta` and named "Mote (beta)", so
it runs beside the stable app, and it shares the stable app's connections,
settings and passwords. `tools/beta-files.sh` makes its launcher, metainfo and
`co.hyprlab.Mote.Beta.yml` from the stable files, and `tools/build-appimage.sh`
and the workflow run it. The RPM and the Debian package stay `mote`, at version
`X.Y.Z~beta.N`, which sorts before `X.Y.Z`, and replace a stable install.

CI's Flatpak bundles are unsigned, for the signing key stays on the Flatout
server, so they are what goes into the repository, not what people download.
Once they are live, `tools/bundle-from-repo.sh X.Y.Z` makes the bundles to
publish from the commits the server signed. Those carry the repository's address
and key, so an install from one receives updates, as an install from the
repository does. They replace CI's on Flatout and on the release.

## Pictures of the interface

With `MOTE_SHOWCASE=<file.png>` set, the app stages a view, writes a picture
of its window and quits, so a change to the interface can be checked without
anyone at the keyboard. `MOTE_SHOWCASE_OPEN` takes comma-separated steps:
`first` selects the first connection, `editor` opens the editor,
`settings` or `settings:<page>` opens Settings and pictures that window,
`about` opens About, `search:<text>` searches for that text, and
`connect:<name>` connects to the connection of that name, after which
`detach` moves its session to a window of its own, `fullscreen` makes it
fullscreen, and `scaling:resize`, `scaling:fit` or `scaling:actual` sets how
it is scaled. `delay:<seconds>` makes the steps after it wait.
Point `XDG_CONFIG_HOME` at a scratch directory to keep it away from your own
connections.

## The app icon

The icon's master is `data/icons/src/default.svg`, drawn on GNOME's 128 pixel
icon grid, and `default.Devel.svg` is the beta's: the same with a hazard stripe.
The macOS app has its own, `macos.svg`, drawn on Apple's icon grid. After
changing any of them, run `tools/gen-app-icons.py` (it needs python3-gobject
with librsvg and pycairo). It copies the GNOME ones into hicolor as the
scalable icons and renders the 256 and 512 pixel PNGs and the README artwork
in `data/repo/`. It renders the macOS one as the 1024 pixel PNG in
`data/icons/macos`, which `tools/build-macos-app.sh` makes the bundle's icon
from. The symbolic icon in `data/icons/hicolor/symbolic/apps` is drawn by
hand. The build embeds the SVGs in the binary, the macOS one in the macOS
build, so About shows the icon without an install.

## Connection robots

A connection is shown as a robot from DiceBear's Pixelbot style (CC0 1.0),
drawn by the app from a table of which cells its eyes and mouth light,
`src/ui/avatar/pixelbot.rs`. `tools/gen-pixelbot.py` writes that table
from the style's definition, `pixelbot.json`, which the `dicebear-styles`
crate and the `@dicebear/pixelbot` npm package carry:

```sh
tools/gen-pixelbot.py ~/.cargo/registry/src/*/dicebear-styles-*/src/pixelbot.json
```

## Tests

```sh
cargo test
cargo clippy --all-targets
```

One test connects to a real server and is skipped unless asked for. It
expects a GNOME Remote Desktop, or any RDP server, with the user name and
password `mote`:

```sh
MOTE_TEST_SERVER=localhost:3399 cargo test -- --ignored live
```

`MOTE_TEST_USER` and `MOTE_TEST_PASSWORD` give another account, and
`MOTE_TEST_EXPECT=refused` expects the server to turn it down. `MOTE_LOG`
sets what it logs; `MOTE_LOG=mote::wire=trace` adds the start of every
message sent and received.

### The clipboard

`MOTE_TEST_CLIPBOARD=<text>` makes the live test offer the server that
text, with HTML and a picture beside it, and fetch whatever is copied on
the remote desktop while it runs (`MOTE_TEST_SECONDS`), files into
`MOTE_TEST_FETCH` (`/tmp/mote-fetch` by default). `MOTE_TEST_FILES` offers
files instead, as paths split by `:`. `MOTE_LOG=mote=debug` logs the
formats each side offers.

### Sound

`MOTE_TEST_SOUND=1` makes the live test play the server's sound, and
`MOTE_TEST_MICROPHONE=1` send the microphone. To keep a test off the
speakers, give it a PulseAudio server of its own in `PULSE_SERVER`: a
private PipeWire with `pipewire-pulse` and a null sink, whose monitor
`parec` records. PipeWire doesn't hand a monitor to a recording stream by
default, so a microphone for the test is a null sink's monitor remapped
with `module-remap-source`.

### Shared folders

`MOTE_TEST_FOLDER=<dir>` makes the live test lend that folder. xrdp shows
it in `~/thinclient_drives`, readable only by the session's user.

### The way there

`MOTE_TEST_SSH=<user>@<host>[:port]` makes the live test go through that
SSH server, where `MOTE_TEST_SERVER` is the computer as the SSH server sees
it, such as `localhost:3389`. It signs in with the agent in
`SSH_AUTH_SOCK`, then the keys in `~/.ssh`, then `MOTE_TEST_SSH_PASSWORD`.
`MOTE_TEST_GATEWAY=<host>:<port>` goes through that RD Gateway instead, as
`MOTE_TEST_GATEWAY_USER` with `MOTE_TEST_GATEWAY_PASSWORD`.
`MOTE_TEST_WAKE=<mac>` sends a Wake-on-LAN packet first. While a session
stays (`MOTE_TEST_SECONDS`), breaking its connection shows it reconnect.

### Monitors

`MOTE_TEST_LAYOUT=<width>x<height>,<width>x<height>` makes the live test ask
for monitors of those sizes side by side after the first frame, the first
of them the primary one. `MOTE_TEST_PICTURE` then holds the whole desktop.

### Measuring performance

`MOTE_LOG=mote::stats=info`, for the app or the live test, logs what a
session's pictures cost every five seconds and when it ends: frames and
kilobytes a second, and as mean/p95/max milliseconds the time to decode an
update, to copy it out for the interface, and from decoding to the paint
that shows it. Without it nothing is timed.

A desktop that changes all the time, the same way every run, makes the
numbers comparable. The private GNOME Remote Desktop shows any program
given in `GRD_CLIENT`, such as GStreamer's moving test pattern:

```sh
GRD_CLIENT='gst-launch-1.0 videotestsrc pattern=smpte horizontal-speed=4 is-live=true
  ! video/x-raw,width=1920,height=1080,framerate=60/1 ! waylandsink fullscreen=true' \
  systemd-run --user --collect --unit=mote-grd -E GRD_CLIENT temp/grd/run.sh
MOTE_TEST_SERVER=localhost:3399 MOTE_TEST_SIZE=1920x1080 MOTE_TEST_SECONDS=12 \
  MOTE_LOG=mote::stats=info cargo test -- --ignored live --nocapture
```

`MOTE_TEST_SIZE` is the desktop to ask for, `MOTE_TEST_SCALE` its scale in
percent (100 by default), `MOTE_TEST_SECONDS` how long to stay.
`pattern=ball` changes a small part of the screen instead of all of it.
`MOTE_TEST_QUALITY` is the connection's codec choice: `best`, `avc420` or
`remotefx`. The log names the codec the server picked; the private GNOME
Remote Desktop has no GPU to encode H.264 on and sends progressive RemoteFX,
so H.264 is measured against xrdp or KRdp.

VNC is measured the same way, through its own live test. `GRD_VNC=1` has
the private GNOME Remote Desktop serve VNC on port 5997, password `mote`,
instead of RDP; it takes one VNC client, and then needs starting again:

```sh
systemd-run --user --collect --unit=mote-grd -E GRD_VNC=1 -E GRD_CLIENT temp/grd/run.sh
MOTE_TEST_VNC=localhost:5997 MOTE_TEST_SECONDS=12 MOTE_TEST_CODEC=best \
  MOTE_LOG=mote::stats=info cargo test vnc::tests::live -- --ignored --nocapture
```

`MOTE_TEST_CODEC` is `best`, `lossless` or `low`; `MOTE_TEST_ENCODING`
asks for one encoding alone (`tight`, `zrle`, `hextile`, `zlib`, `h264` or
`raw`), and `MOTE_TEST_SIZE` for a desktop size.
