# Roadmap

Mote is built in sections. Each section ends with something that builds,
runs and has been tried against a real Linux RDP or VNC server before the
next one starts, and each can change shape once the one before it has been used.

## Servers

Every section that touches the protocol is checked against:

- **GNOME Remote Desktop** (GNOME 46 or later), both kinds: Desktop Sharing in
  a user's session, and Remote Login, the system service that signs a user in
  and then hands the client to the new session with a Server Redirection PDU.
  It requires the graphics pipeline, and NLA.
- **xrdp** 0.10 or later, with and without the graphics pipeline (H.264 and
  RemoteFX), and with its PulseAudio and PipeWire sound modules.
- **KRdp** on KDE Plasma 6.

Every VNC section, from 13 on, is checked against:

- **GNOME Remote Desktop's VNC**, in Desktop Sharing and headless (Remote
  Login is RDP only): anonymous TLS (security type 18) around VNC
  Authentication, or a prompt the desktop's user answers.
- **TigerVNC** 1.15, `Xvnc` and `x0vncserver`: VNC Authentication, VeNCrypt
  with and without a certificate, and RA2.
- **wayvnc** 0.9 on Sway, built on neatvnc: VeNCrypt with a certificate,
  RSA-AES, Open H.264.
- **KRfb** on KDE Plasma 6, and **x11vnc**, both built on libvncserver.

## Protocol engine

[IronRDP](https://github.com/Devolutions/IronRDP), the RDP stack Devolutions
writes in Rust. It has a client connector with TLS and CredSSP (NLA), the
graphics pipeline with OpenH264 decoding, the clipboard, display control,
sound, drive redirection, smart cards and RD Gateway, all as separate crates.
Mote names those crates one by one (`ironrdp-connector` 0.12,
`ironrdp-session` 0.13 and the rest of that release), because the `ironrdp`
umbrella crate pins older versions of them. Each session runs on a thread of
its own, and nothing in the engine needs C bindings.

Where IronRDP falls short today, Mote works around it, and each workaround
is to go upstream:

- GNOME Remote Desktop's Server Redirection PDU carries version 1 in its
  Share Control Header, as everything FreeRDP builds does; IronRDP accepts
  only 0 there. Mote rewrites the version before IronRDP reads the PDU.
- The handover a redirection leads to needs RDSTLS, which IronRDP's
  connector doesn't offer. Mote asks for it and does its exchange, then
  lets the connector carry on.
- IronRDP's sound channel answers the server's Training PDU with eight
  bytes less than the `wPackSize` the server sent; GNOME Remote Desktop
  checks the size and sends no sound after it. Mote answers the training
  itself.
- IronRDP's backend for shared folders reads xrdp's requests more strictly
  than Windows and FreeRDP do. Mote answers the directory listing classes
  it leaves out, turns xrdp's forward slashes into backslashes, lets every
  handle delete, and removes an entry when the handle that marked it
  closes rather than the last one, as xrdp leaves a handle open.
- IronRDP's reactivation sequence takes every message that comes while
  it runs for its own. xrdp goes on sending on its static channels during
  the reactivation after a change of monitors, and the sequence fails on
  the first such message. Mote holds those messages back and takes them up
  once the sequence ends.
- IronRDP's RD Gateway client turns on the network client of its NTLM
  library, which brings aws-lc into rustls beside ring. With two providers
  rustls can't choose one, so Mote names ring as the process's own.

FreeRDP 3, the engine Remmina uses, was the alternative. It supports more
devices (USB, serial and parallel ports), at the cost of a C API surface, a
callback-driven event loop and a larger Flatpak. Where IronRDP comes up short
on something a section needs, that section adds it upstream.

## VNC engine

Mote's own, in Rust, in `src/vnc/`. No Rust VNC client covers what Linux
servers ask for, as IronRDP does for RDP. vnc-rs, the one maintained crate,
signs in with VNC Authentication alone, with no encrypted security type,
and its connector keeps the stream to itself, so TLS can't be added part of
the way through the handshake without redesigning it. gtk-vnc and
libvncclient have all of it, in C. RFB is a small protocol, and the rest of
what a session needs is already here: the session thread, trust on first
use, the SSH tunnel, reconnection, the frame buffers, the H.264 decoders,
the clipboard and the keyring.

The handshake runs over a stream the engine holds, so the engine decides
when the stream becomes TLS, as Mote does around IronRDP's connector.
Certificates go
through rustls and the trust store RDP uses. GNOME Remote Desktop and
TigerVNC default to anonymous TLS, with Diffie-Hellman and no certificate,
which rustls doesn't implement; for that alone Mote loads OpenSSL's
`libssl.so.3` at run time, as it loads FFmpeg, and nothing links it. A
headless GNOME Remote Desktop signed in over it with
`ADH-AES256-GCM-SHA384` on Fedora 44.

## Sections

| # | Section | State |
| --- | --- | --- |
| 1 | Foundation | Done |
| 2 | Connections | Done |
| 3 | First session | Done |
| 4 | HiDPI and resizing | Done |
| 5 | Clipboard | Done |
| 6 | Graphics pipeline | Done |
| 7 | Audio | Done |
| 8 | Device redirection | Done |
| 9 | Network | Done |
| 10 | Session controls | Done |
| 11 | Settings and polish | Done |
| 12 | Packaging | Done |
| 13 | VNC first session | Done |
| 14 | VNC security | Done |
| 15 | VNC display | Done |
| 16 | VNC clipboard | Planned |
| 17 | VNC connections | Planned |

### 1. Foundation

The repository, its conventions and tooling, and an app that opens a window:
relm4 on GTK 4 and libadwaita, gettext for translations, tracing for logs.

### 2. Connections

- The connection profile: host, port, user name, domain, and every
  per-connection setting the later sections add, stored as TOML in
  `~/.config/mote`, which the Flatpak, the packages and the AppImage share.
- Passwords in the desktop's Secret Service (the GNOME keyring), from the
  Flatpak too, never in the profile.
- The connection list: search, groups, last-used order, and a search field
  that takes `host[:port]`: Enter on an address nothing matches starts a new
  connection to it. Connecting straight from it comes with section 3.
- The connection editor, as a libadwaita dialog with preferences pages, and
  Delete beside Save for a connection already saved.
- Other addresses of the same computer on a connection, such as its local
  name and its name on a VPN, put in order by dragging them or with Alt+Up
  and Alt+Down. Connecting tries them in that order, 5 seconds for
  each but the last, and goes on when one can't be reached or something other
  than an RDP server answers there. An answer from the server ends the search:
  a refused password or a rejected certificate doesn't try the next one. A
  certificate trusted under one of them counts for all of them, and through an
  SSH tunnel or an RD Gateway they are addresses as seen from its far end.
- Import of `.rdp` files and of Remmina's `.remmina` profiles, and export
  to `.rdp`. A connection's first other address is the file's `alternate full
  address`; the format has no room for the rest, so export leaves them out.
- Scan Network, in the main menu, lists the RDP servers on the local networks.
  Add opens the connection editor on one with its address filled in, and
  Cancel there saves nothing. It covers ports 3389 and 3390 on the private IPv4 networks
  this computer is on, as a /24 at the widest, and leaves out this computer,
  VPNs and container bridges. Its Networks page adds others to scan and keeps
  them in Settings: a private, link-local or shared range, as a /16 at the
  widest, or one address. Linux servers don't announce themselves, so it
  tries each address, and runs only when asked to. A server is asked for its
  certificate, which says which server it is, and nothing more: no sign-in
  is attempted and GNOME Remote Login starts no display, but each server logs
  a few errors per scan.

### 3. First session

- Connecting: TCP, TLS, NLA, and server redirection, which GNOME Remote Login
  depends on.
- Certificate trust on first use, with the fingerprint shown, and a warning
  when a known server's certificate changes.
- A credentials prompt when the profile has none or the server refuses them.
- The session view: frames into a texture, redrawing only damaged regions.
- Keyboard input as scancodes taken from the hardware keycode, so the remote
  layout decides what a key types; pointer, buttons, smooth and discrete
  scrolling; the server's cursor shapes.
- Disconnect, reconnect, and readable errors that name the likely cause on a
  Linux server and where to fix it: GNOME Remote Login without credentials,
  the wrong port, a server without a certificate, xrdp set to the old RDP
  security, outdated TLS, a password from the wrong place, and the reasons
  a server gives for ending a session, such as another connection taking
  it over.
- xrdp signs in with the profile's account, without its own login screen.
- Where a session lives: in the main window beside the sidebar, with the
  connection's row marked; moved to a window of its own and back; and
  fullscreen, with the header shown again at the screen's top edge.

Built, and tried against a headless GNOME Remote Desktop: the certificate
question, sign-in and a refused password, typing and clicking on the remote
desktop, and a server that stops, isn't running, or closes every connection
at once. Then against real servers on Fedora 44: GNOME Remote Login, through
the handover to its login screen and on to the user's session; xrdp 0.10.6
with Xfce, signing in without its login screen, and set to the old RDP
security or without a certificate; and KRdp 6.6.4, which turns the session
down until H.264 arrives in section 6.

GNOME Remote Desktop refuses a client without the graphics pipeline, so its
basics arrived here rather than in section 6: the pipeline's channel, with
the codecs IronRDP decodes without H.264.

### 4. HiDPI and resizing

- The session sized to the window and resized with it (Display Control,
  MS-RDPEDISP).
- Desktop and device scale factors taken from the monitor, fractional scales
  included, and updated when the window moves between monitors.
- Scaling modes: resize the remote desktop, fit to the window, or one remote
  pixel per device pixel.
- Fullscreen.

Built: the desktop is asked for in device pixels with the screen's scale,
and Display Control resizes it with the window, a moment after the window
settles; the pointer is drawn at the same scale. A server that keeps a size
of its own, as KRdp keeps its screen's, or that has no Display Control, is
scaled to fit the window instead. The scaling mode is chosen
per connection, in the editor or the session's menu. Fullscreen arrived with
section 3. Tried at 1×, 1.25× and 2× against a headless GNOME Remote
Desktop, and resizing against GNOME Remote Login and xrdp, which reopens its
graphics channel after a resize. Not yet tried: moving a window between
screens of different scales.

### 5. Clipboard

Text, rich text, images and files in both directions (MS-RDPECLIP), through
GDK's clipboard, with a per-connection switch.

Built on IronRDP's clipboard channel. What is copied on either side is
offered to the other and fetched only when something pastes it: text, HTML,
pictures (PNG here; PNG and DIB on the wire) and files with their folders.
Files pasted here from a remote desktop are fetched into the cache
directory, which the host sees from inside the Flatpak too, and cleared out
when the app next starts. Wayland gives a window the clipboard only while
it has the focus, so something copied elsewhere reaches the server once the
session's window is focused again. The switch is in the editor, and comes
in and out with `.rdp` files (`redirectclipboard`) and Remmina profiles
(`disableclipboard`).

The server reads the local clipboard whenever it asks, with nothing done
here, so sharing is off for a new connection and is turned on in the editor
or for every new connection in Settings. Files copied here have a switch of
their own, off unless turned on, since offering them gives the server their
paths and lets it read them; with it off, a copy of files offers the
server nothing, not even its paths as text, whatever the interface sends. A connection's first
session that shares the clipboard says once that the server can read it,
unless that notice is off in Settings.

Tried against all three servers, which differ:

| Server | Text | HTML | Pictures | Files |
| --- | --- | --- | --- | --- |
| GNOME Remote Desktop | Both ways | Neither way | Both ways | Both ways, with folders |
| xrdp | Both ways | Neither way | Both ways | Both ways, without folders |
| KRdp | Both ways | Neither way | Neither way | Neither way |

- GNOME Remote Desktop converts HTML with FreeRDP's WinPR, whose conversion
  from Windows' HTML Format to `text/html` can't succeed in WinPR 3.32, the
  version Fedora 44 has: HTML fails for every client, and the plain text
  beside it is what pastes. It fails a request that comes while it answers
  another, so Mote asks for one thing at a time. It gives its applications a
  picture as PNG when the client offers PNG, and as BMP otherwise, so Mote
  offers both.
- xrdp offers text and BMP pictures, nothing richer, and leaves folders out
  when files are copied either way. It passes Windows' CRLF line endings on
  to its applications, where GNOME Remote Desktop and KRdp turn them into
  LF, so Mote sends text with the LF endings it has. When what was copied
  there is gone before it is pasted here, xrdp never answers; a paste waits
  30 seconds at most.
- KRdp shares plain text only, and offers a picture as empty text. It
  announces back what it has just taken from the client; an announcement
  within a second of that, holding nothing new, is taken as the echo and
  leaves the local clipboard alone.

Copying a picture file to GNOME Remote Login froze Carbon's screen. It runs
Copyous, a clipboard history extension that previews a copied picture with
GdkPixbuf on GNOME Shell's own thread. GNOME Remote Desktop serves the
file from a FUSE mount that asks the client for each read as it comes, 4
kilobytes per round trip, and Carbon is on Wi-Fi: the shell waited for
the whole picture to cross. Any client would do the same. Copyous leaves a
file alone that matches its `file-preview-exclusion-patterns`, such as
`file:///run/user/*/gnome-remote-desktop/**`.

Tried in the app against the headless GNOME Remote Desktop, with a GTK
application on each side, and through the engine against all three. Not
yet tried: the Flatpak, where local files come through the file transfer
portal.

### 6. Graphics pipeline

- The graphics pipeline (MS-RDPEGFX), which section 3 started: H.264
  decoding, as AVC420 and AVC444.
- Decoding off the main thread, upload straight to a GPU texture, frames paced
  to the monitor's refresh.
- Quality presets that set color depth, codecs and the server's visual
  effects, with a custom option.
- A measurement harness, so performance changes are compared by numbers.

Built so far: H.264, offered as AVC444 and AVC420, through FFmpeg, the
system's OpenH264, or Cisco's OpenH264 downloaded on request from Settings,
all loaded at run time; FFmpeg is what the Flatpak's GNOME runtime provides,
and was tried inside it. Fedora's FFmpeg has no H.264 decoder of its own,
only OpenH264 wrapped to hold each picture back for reordering, which left
GNOME Remote Desktop's AVC444 blank; that FFmpeg is passed over for
OpenH264 called directly. Decoding runs on the session's own thread.

What each server sends, as measured: KRdp sends only AVC420, and turns down
a client without H.264. xrdp sends AVC420 when it has an encoder (Fedora's
Xfce install comes with the stand-in `noopenh264`, and then xrdp sends
lossless Planar for still pictures and RemoteFX for motion). GNOME Remote
Desktop sends AVC444 or AVC420 only where it can encode on the GPU; Carbon
and the headless test server can't, and get progressive RemoteFX whatever
the client offers.

When a server offered H.264 sends none within its first 3 seconds of
picture, or ends the session for want of an encoder, a toast or the ended
page offers How to Fix: what to install on the server, by server,
distribution and, for GNOME Remote Desktop, graphics card. The server is
guessed from its certificate (GNOME's settings name it GNOME, Fedora's xrdp
XRDP, KRdp keeps OpenSSL's defaults) and from Remote Login's redirection;
an SSH tunnel that ends on the computer reads its `/etc/os-release` and
graphics vendor. What is not found is chosen in the dialog, and kept with
the connection. Missing H.264 Warning in Settings turns the toast off. The advice was checked against each distribution's packages
in November 2026 and lives in `src/ui/session/h264_help/advice.rs`.

The measurement harness is in place (BUILDING.md, under Measuring
performance). Its first numbers, at 1920×1080 on the development machine:

| Step | Time per frame |
| --- | --- |
| H.264, FFmpeg's own decoder (the Flatpak's) | 2.6 ms |
| H.264, OpenH264 | 4 ms |
| IronRDP turning the picture into RGB and composing it | 1.5 to 3 ms |
| Progressive RemoteFX, all of it | 4 ms |
| Copying the whole desktop out for the interface | 0.8 ms |

Hardware decoding would save only the decoder's share, and the picture
would have to come back from the GPU for IronRDP to compose it; it waits
until a slower machine shows a need. The copy is gone for most frames:
frames come from a few buffers used in turn, each brought up to date with
only what changed, and a small change costs 0.01 ms. Every frame goes to
GTK as it comes, replacing one not yet painted, so each refresh of the
monitor shows the newest picture.

Quality presets became a choice of codec per connection: Best Available (AVC444
where the server has it), AVC420, or RemoteFX. Color depth applies only to
the old bitmap updates, which none of these servers send once the pipeline
is offered, and they ignore the visual effect flags (xrdp reads one, to
leave out its own login screen's background), so neither is offered.

The network type goes to the server too, LAN unless the connection says
otherwise, and comes in and out with `.rdp` and Remmina files. Of these
servers only xrdp acts on it, by setting its H.264 bitrate: at 1920×1080 with
the screen in motion, LAN took 1.8 MB/s at 43 frames a second, low-speed
broadband 0.2 MB/s at 7, a modem 0.1 MB/s at 2.5. Automatic detection isn't
offered: IronRDP can't answer the server's measurements, and xrdp would take
it as LAN.

### 7. Audio

Playback (MS-RDPEA) through PipeWire, the microphone (MS-RDPEAI), and a
per-connection choice of here, on the remote machine, or off.

Built on IronRDP's sound and audio input channels, both the static one xrdp
uses and the dynamic one GNOME Remote Desktop uses. Sound is played, and the
microphone recorded, through PulseAudio's simple API as PipeWire serves it:
it is what a Flatpak reaches with `--socket=pulseaudio`, where PipeWire's
own socket is out of reach. Each stream runs on a thread of its own, about
100 ms ahead of the speakers. Sound comes as Opus where the server offers
it, decoded with libopus, and as PCM otherwise: from GNOME Remote Desktop,
14 kilobytes a second against 176. The microphone goes as PCM.

Sound and the microphone are two switches in the editor, the microphone off
until turned on. Sound played on the remote machine and no sound at all,
which Windows tells apart, are the same to these servers: what isn't sent
stays where it is, so the choice is here or not. Both come in and out with
`.rdp` files (`audiomode`, `audiocapturemode`) and Remmina profiles
(`sound`, `microphone`).

Tried against the three servers, through a private PipeWire so nothing
reached the speakers:

| Server | Sound | Microphone |
| --- | --- | --- |
| GNOME Remote Desktop | Opus, or PCM | PCM |
| xrdp | PCM | PCM |
| KRdp | No | No |

xrdp needs `pipewire-module-xrdp` (or the PulseAudio one) in the session,
which Fedora doesn't package; built from neutrinolabs' source on the test
machine, it gave both. KRdp opens no sound channel. Not yet tried in the
app itself, or in the Flatpak, which needs `--socket=pulseaudio`.

### 8. Device redirection

Shared folders (MS-RDPEFS), smart cards and printers. USB, serial and
parallel port redirection are not in IronRDP; this section decides whether
they are worth adding.

Built: one shared folder per connection, chosen in the editor and read from
Remmina profiles (`sharefolder`), through IronRDP's backend for Unix, which
resolves every path beneath the folder and follows no symbolic links. One
folder is what that backend serves, and what Remmina offers. A folder that
is gone when connecting isn't offered.

Of the three servers only xrdp opens the device channel; GNOME Remote
Desktop and KRdp never do. With xrdp the folder is in `~/thinclient_drives`,
and was tried there by listing, reading, writing, renaming, copying and
deleting files and folders, and by trying to climb out of it with `..`.
xrdp's requests needed the workarounds listed under Protocol engine.

Decided against, for these servers:

- Printers: xrdp turns a redirected printer down, and the others don't open
  the channel. IronRDP's spooler, which hands jobs to `lp`, is left out.
- Smart cards: IronRDP serves them only on Windows. A PC/SC backend for
  Linux would help xrdp alone; it waits until someone needs it.
- USB, serial and parallel ports: none of the three servers takes them.

### 9. Network

- SSH tunnels, with keys from the SSH agent: the usual way into a Linux box
  that exposes only SSH.
- RD Gateway.
- Wake-on-LAN before connecting.
- Automatic reconnection after a dropped network.

Built: a connection goes straight to the computer, through an SSH server or
through an RD Gateway, and can wake the computer first. The tunnel and the
gateway are set in the editor, one or the other, with their passwords in
the keyring beside the connection's.

- The SSH tunnel forwards the RDP port the way `ssh -L` does, through
  russh. It signs in with the SSH agent's keys, then the keys in `~/.ssh`
  without a passphrase, then the tunnel's password. The server's key is
  checked against `~/.ssh/known_hosts`; one that isn't there is asked about
  with the fingerprint `ssh-keygen -l` shows, and one that differs from it
  ends the connection, as `ssh` does. Host aliases in `~/.ssh/config` are
  not read. The user name, left empty, is the local one.
- The RD Gateway is IronRDP's client for it, over a WebSocket or the older
  pair of HTTP requests. It signs in with an account of its own or the
  connection's. A certificate no authority on the system vouches for is
  asked about, as a server's is.
- Wake-on-LAN broadcasts the magic packet to ports 9 and 7, then tries the
  computer every few seconds for two minutes, sending the packet again
  every 15 seconds, while the window says it is waking the computer.
- A session whose connection drops, rather than one the server ends, is
  made again for up to two minutes, to the server first asked for and at
  the size it last had. TCP keepalive and a send timeout notice a network
  gone quiet in about 20 seconds, where TCP alone takes a quarter of an
  hour.

`.rdp` files carry the gateway (`gatewayhostname`, `gatewayusagemethod`,
`gatewayusername`) both ways; Remmina profiles bring the tunnel
(`ssh_tunnel_*`) and the gateway (`gateway_server`, `gateway_username`).

Tried:

- The SSH tunnel through the xrdp machine to its own xrdp: with the agent,
  with a key file and no agent, with a wrong password, with a host key not
  yet known, and to a port nothing listens on.
- The RD Gateway as rdpgw, an open-source gateway, in a container in front
  of xrdp: in with Basic authentication, a wrong password, and a computer
  the gateway doesn't allow. rdpgw reads NTLM under "Negotiate" as bare
  NTLM, where IronRDP sends it wrapped in SPNEGO as Windows expects, so
  NTLM through rdpgw fails. A Windows RD Gateway hasn't been tried.
- Wake-on-LAN's packet, as another machine on the network received it. A
  computer actually asleep hasn't been woken yet.
- Reconnection to xrdp after its end of the connection was reset, and after
  its packets were dropped for 40 seconds: noticed in 19 seconds, back in
  the same session once they went through. In the app, a private GNOME
  Remote Desktop stopped and started again.

### 10. Session controls

- Keyboard grab that keeps GNOME's own shortcuts out of the way while the
  session has focus, with a key to release it.
- Sending key combinations the local desktop would otherwise take:
  Ctrl+Alt+Del, Ctrl+Alt+F1 to F12, Super.
- Screenshots, view-only mode, multiple monitors.
- Commands run before connecting and after disconnecting.

Built, in the session menu and the editor:

- The keyboard grab: while the remote desktop has focus, this computer's
  shortcuts are held back through the compositor's shortcut inhibitor, so
  Super, Alt+Tab and the rest go to the session. As in Remmina, it follows
  focus with nothing to press: the desktop takes the keyboard as soon as it
  shows and whenever the pointer comes onto it, and lets go when the
  pointer leaves it (at the end of a drag that leaves it) or the window or
  the keyboard goes elsewhere. GNOME asks once whether
  to allow it, and gives the shortcuts back on Super+Esc; a toast says so
  the first time, unless Keyboard Grab Notice is off in Settings. Grab Keyboard in the menu turns it off for the session.
- Send Keys: Ctrl+Alt+Delete, Super, Alt+Tab, Print Screen and Ctrl+Alt+F1
  to F12, for when the grab is off.
- View Only, in the menu and the editor (Remmina's `viewonly`): keys and
  the pointer stay here. What the menu sets is kept.
- Open Fullscreen and Open in New Window, in the editor: a session to the
  connection starts in a window of its own, fullscreen, or both. A session
  opening fullscreen waits for the window to be so before it asks for the
  desktop's size, so Fit and Actual Size get the screen's. Remmina's
  fullscreen `viewmode` and the `.rdp` file's `screen mode id` come across.
- Take Screenshot saves the desktop, at its full size, as a PNG in the
  Screenshots folder of Pictures, where GNOME's own go, named for the
  connection and the time.
- Fullscreen on All Monitors, with more than one monitor: the remote
  desktop gets a monitor for each one here, in the same arrangement, over
  Display Control, and each monitor beyond the session window's shows its
  part in a fullscreen window of its own. Monitors with different scales go
  side by side, as their pixels don't line up. Leaving fullscreen brings it
  back to one. A server without Display Control, or with room for fewer
  monitors, says so and stays on one. Spanning one window over several
  monitors, as Remmina does on X11, isn't something Wayland allows.
- Commands before connecting and after disconnecting, run by `sh -c` with
  the connection in `MOTE_HOST`, `MOTE_PORT`, `MOTE_USER`, `MOTE_NAME` and
  `MOTE_GROUP` rather than pasted into the text. The connection waits for
  the first, and stops with its last line of errors when it fails.
  Remmina's `precommand` and `postcommand` come across, with `%h`, `%u`,
  `%p` and `%g` turned into those variables.

Remmina's tabs, floating toolbar and scrolled view are already here in
other forms: a session per connection in the sidebar, the header that
comes down at the top edge in fullscreen, and Actual Size.

Tried in a headless GNOME with two monitors against a private GNOME Remote
Desktop: the grab granted, keys typed and blocked in view-only, a
screenshot, a window dragged from one monitor to the other, back to one,
and both commands, the first failing as well. xrdp and GNOME Remote Desktop
both took a layout of two monitors in the live test; KRdp has no Display
Control. Not tried yet on a real desktop with two monitors, where GNOME
asks before granting the grab.

### 11. Settings and polish

App preferences, default connection settings, a keyboard shortcuts dialog,
accessibility review, and translations.

Built:

- New Connection Defaults in Settings sets the scaling, codec, fullscreen,
  new window, clipboard, sound and microphone a new connection starts with,
  beside its port and account.
- A Sessions page holds the keyboard layout, the keyboard grab and
  reconnecting. The layout follows this computer's (the one GDK reports
  active) unless one is chosen, and goes to the server as a Windows layout
  ID; xrdp sets the session's X layout by it, where it always set US
  English before, and the other servers keep their own.
- Keyboard Shortcuts, on Ctrl+? and in the main menu, in a dialog of rows
  rather than libadwaita's shortcuts dialog, which needs 1.8 where Debian 13
  has 1.7; the same list is in [KEYBOARD_SHORTCUTS.md](KEYBOARD_SHORTCUTS.md).
- Right Ctrl, alone, lets go of the keyboard, as Remmina's host key does:
  without it a keyboard alone couldn't leave a session, which takes every
  key. With another key it goes to the server. In fullscreen it shows the
  header, which stays until the desktop has the keyboard again: a click on
  it, or the pointer leaving it and coming back. Resting on it doesn't take
  the keyboard back.
- The accessibility review, through the AT-SPI tree in a headless GNOME:
  every icon-only button has a name, the sidebars' rows are named for
  their connection or category rather than by their tooltip, group
  headings are named, and the desktop is named for its connection. What is
  left unnamed is inside libadwaita's combo and expander rows, which name
  themselves.
- Translations: `po/mote.pot`, the tools that make and compile it, and
  `po/README.md` for translators, as Hylki has. Languages come from the
  people who speak them.

- Connections are shown as robots, DiceBear's Pixelbot (CC0), rather than
  initials: the one a connection's id picks, another chosen in the editor,
  more of them on request, or an image of the user's own, kept as a copy
  no larger than it is shown. Robots are drawn rather than kept as images,
  so they cost no memory per connection.

Tried: xrdp took German (`0x407`) and US English, as `setxkbmap -query` in
the session showed; Right Ctrl alone let go in a session, and Right Ctrl+A
selected everything on the server; a copy of a real `settings.toml` loaded
unchanged; a German catalogue built from the tree showed in the app.

### 12. Packaging

A Flatpak on the GNOME runtime, AppStream metainfo, the app icon, a desktop
file, and an RPM. The Flatpak needs the codecs-extra extension for H.264
and `--socket=pulseaudio` for sound and the microphone; the SSH tunnel
needs `--socket=ssh-auth` for the agent and `--filesystem=~/.ssh:ro` for
`known_hosts` and the key files, and screenshots
`--filesystem=xdg-pictures`. Commands before and after a session run
inside the sandbox.

Built, as Hylki's are (the steps are in [BUILDING.md](BUILDING.md#flatpak)):

- `co.hyprlab.Mote.yml`, a Flatpak on the GNOME 50 runtime, built offline
  from vendored crates. Beside the permissions above it reads Remmina's
  profiles, read-only, for the import, and needs nothing for passwords or
  a shared folder, which go through portals. codecs-extra comes with the
  25.08 runtime underneath, which Flatpak installs along with it from
  Flathub, and its libavcodec is the one the runtime's linker finds first.
- `data/co.hyprlab.Mote.metainfo.xml`, which `appstreamcli` passes, with
  the icon's colors for branding and `data/repo/screenshot.png`; its text
  goes into the translation template.
- `packaging/fedora/mote.spec` and `tools/build-packages.sh`, an RPM of the
  host's release build. The libraries Mote opens itself (PulseAudio's simple
  API, Opus, FFmpeg, OpenH264) are recommended, as rpm can't see them
  linked.

- An AppImage (`tools/build-appimage.sh`) and a Debian package
  (`tools/build-packages.sh deb`), as Hylki builds them. The AppImage
  bundles every library but FFmpeg, whose H.264 decoder isn't Mote's to
  hand out; it uses the system's where there is one, and Cisco's OpenH264
  otherwise. The deb is built on Debian 13, for it and Ubuntu 25.04 on.
  The shortcuts dialog went back to libadwaita 1.7 for it.
- `.github/workflows/build-packages.yml`, which builds all of them for
  x86_64 and ARM64, the ARM64 ones on GitHub's arm64 runners, checks the
  RPM and the deb install, and drafts a release from a tag.

Tried: the Flatpak, installed for the user and run in a headless GNOME,
signed in to a private GNOME Remote Desktop and showed it, found FFmpeg
through codecs-extra, and found libopus and libpulse-simple in the runtime.
The RPM installed into a clean Fedora 44 and the deb into a clean Debian
13, with every library they link found and their recommendations pulled
in. The AppImage connected to a private GNOME Remote Desktop, and to xrdp,
whose H.264 it decoded through the system's FFmpeg. The workflow passes
actionlint. Not tried yet: any of them on a real desktop, H.264 from xrdp
in the Flatpak, the ARM64 builds, which need the workflow to run on
GitHub, and a release from a tag.

### 13. VNC first session

- Two protocols: a connection is RDP or VNC, existing profiles reading as
  RDP, with the port following the choice (3389, 5900). The interface
  moves to types of its own for frames, the cursor, input and the session's
  events, the RDP engine turning them into IronRDP's, and the SSH tunnel,
  Wake-on-LAN, reconnection, the frame buffers and the H.264 decoders are
  shared. This is tried against the RDP servers before VNC starts.
- The handshake: RFB 3.3, 3.7 and 3.8; None and VNC Authentication; a
  32-bit pixel format laid out as the frame buffers are.
- Raw, CopyRect, Hextile, Zlib, ZRLE and Tight with JPEG; LastRect, the
  server's cursor, DesktopSize, DesktopName and ExtendedDesktopSize. TRLE
  is left out: only RealVNC's server sends it.
- Keys as keysyms, and as QEMU extended key events where the server takes
  them, so the remote layout decides what a key types. Five buttons, back
  and forward through ExtendedMouseButtons, and smooth scrolling gathered
  into wheel steps.
- Disconnect, reconnect, and errors that name the cause: a refused
  password, TigerVNC's lock-out after failed attempts, a GNOME prompt
  declined or left unanswered, something other than a VNC server, and
  VNC Authentication's limit of 8 characters.
- Limits on sizes and decompression, and the decoders fed noise.

Built:

- The connection editor has a Protocol row. A VNC connection takes a
  password alone, and its editor leaves out the user name, the domain, the
  codec, the connection type, the RD Gateway and Sharing; the port moves
  to 5900 with it, unless another was set. The sidebar and the
  connection's page say VNC.
- The session's interface moved out of the RDP engine into types of its
  own (`src/remote`), shared by both engines, with the way to the server,
  the frame buffers and the reasons a session ends.
- The engine (`src/vnc`): the handshake, with None and VNC Authentication;
  every encoding above, decoded on the session's thread as the update
  arrives; the next update asked for as soon as one has been read; keys,
  buttons and the wheel; the server's pointer and its bell; reconnection.
  A server that offers only security types still to come is named with
  them.
- Seeded tests feed the message reader and every decoder thousands of
  random and mutated messages, compressed noise among them, and expect an
  error and never a panic. Mote is one binary, which `cargo fuzz` can't
  reach into, so these stand in for fuzzing.

Tried against TigerVNC 1.16.2's `Xvnc` (`temp/vnc/run.sh`): signing in, a
refused password, typing through QEMU extended key events, and each
encoding asked for alone, ZRLE, Hextile and Zlib matching Raw pixel for
pixel and Tight's JPEG within 14 levels of it. Pointed at an RDP server's
port, the engine gives up after 10 seconds of silence. In the app, against
Linux Mint's VNC server on another machine, which offers VNC
Authentication alone: signing in, the picture, typing and the pointer.
Not yet tried: KRfb, x11vnc, wayvnc, and the menu's key combinations.

### 14. VNC security

- The strongest security type the server offers: RA2_256 and RA2, VeNCrypt
  with a certificate, anonymous TLS (type 18 and VeNCrypt's TLS subtypes),
  then RA2ne and Apple DH, which protect the password alone.
- A certificate, or RA2's RSA key by its fingerprint, asked about as an RDP
  server's certificate is. Anonymous TLS is encrypted without the server
  being verified, and says so.
- VeNCrypt Plain and RA2 send the user name with the password.
- A server that offers no encryption stops the connection before the
  password goes out, with what to change on the server and Allow
  Unencrypted for the connection.

Built:

- RSA-AES in its four kinds (RA2, RA2_256, RA2ne, RA2ne_256), VeNCrypt
  with its certificate, anonymous TLS and Plain subtypes, GNOME's anonymous
  TLS (type 18), and Apple's Diffie-Hellman, beside None and VNC
  Authentication. They are taken in that order of strength, RSA-AES first
  as TigerVNC and wayvnc offer it beside VeNCrypt and its key says which
  server it is; taking the same one each time keeps the identity trusted
  the same.
- Anonymous TLS goes through OpenSSL 3, loaded at run time; a computer
  without it says so when a server needs it.
- A certificate or an RSA key is asked about before anything secret goes
  out, by its SHA-256 fingerprint, and kept among the trusted
  certificates; a key and a certificate of one server are trusted apart.
- Allow Unencrypted, a switch in the editor for VNC connections and a
  button on the page that stops one, lets a session go with the password
  alone protected, or nothing at all. A connection's first session that is
  less than encrypted by a trusted server says so, once.
- A VNC connection takes a user name again, for VeNCrypt Plain, RSA-AES
  and Apple's sign-in, which want one; it is never required.

RFB's description of RSA-AES gives its random numbers as 16 bytes; for
RA2_256 and RA2ne_256, TigerVNC and wayvnc's neatvnc both send and expect
32, and Mote does as they do.

Tried against TigerVNC 1.16.2 (RA2_256, RA2, RA2ne, X509Vnc, TLSVnc and
VNC Authentication), wayvnc 0.9.1 (RA2_256, RA2, X509Plain and Apple's
Diffie-Hellman, `temp/wayvnc/run.sh`) and a headless GNOME Remote Desktop
50 (type 18): signing in, a wrong password, a missing user name, the
question of trust, and the stop before an unencrypted server. Not yet
tried: the Flatpak, where OpenSSL comes from the runtime; KRfb and
x11vnc; and GNOME's Desktop Sharing with both RDP and VNC on, where the
headless server stops taking RDP connections once a VNC client leaves.

### 15. VNC display

- Resizing with the window through SetDesktopSize where the server takes
  it, and Fit to the window where it doesn't; several monitors as
  ExtendedDesktopSize screens.
- Continuous updates and Fence where offered; otherwise the next update is
  asked for as soon as one is read, not once it is painted.
- The codec choice per protocol: the order of encodings, Tight's JPEG
  quality and compression, and a lossless choice.
- Open H.264, which wayvnc sends, through the decoders RDP uses.
- Numbers from the measurement harness for each encoding.

Built:

- The desktop takes the window's size, at the start and as the window
  changes, where the server says it can change its screens; elsewhere it
  is scaled to fit. Fullscreen on All Monitors asks for a screen per
  monitor. A server that passes the change on to someone else to make, as
  wayvnc does to its compositor, is given time to make it. TigerVNC and
  GNOME say they can in their first update, neatvnc once a frame shows its
  layout, so a server is taken to keep its size only after three updates.
- Continuous updates, where offered, with the server's fences answered:
  the server sends changes as they happen. Elsewhere the next update is
  asked for as each one is read.
- A codec choice of its own for VNC in the editor: Best Available (H.264
  where the server has it, else Tight with JPEG at quality 8), Lossless
  (ZRLE first, and Tight without JPEG, which it sends only to a client that
  names a quality) and Low Bandwidth (JPEG at quality 3, compressed
  harder).
- Open H.264, each rectangle with a decoder of its own, from the H.264
  sources RDP uses.

Measured at 1920x1080 on a private GNOME Remote Desktop's VNC, which sends
30 frames a second at most, with GStreamer's moving test pattern filling
the screen (`MOTE_LOG=mote::stats=info`, BUILDING.md):

| Codec | Data | Decoding per frame, mean and p95 |
| --- | --- | --- |
| Best Available (Tight, JPEG) | 3.4 MB/s | 3.0 and 3.6 ms |
| Lossless (ZRLE) | 9.5 MB/s | 5.4 and 6.6 ms |
| Low Bandwidth | 1.6 MB/s | 3.8 and 6.5 ms |

Copying a frame out for the interface took 0.8 ms in each.

Tried: resizing on TigerVNC, wayvnc (headless) and GNOME Remote Desktop;
two monitors on all three, as screens side by side; continuous updates on
TigerVNC and wayvnc (GNOME doesn't offer them); and in the app, TigerVNC's
desktop taking the window's size. H.264 decoding was checked against a
picture FFmpeg encoded with VA-API, colors and cropping included, but no
server here sends it: Fedora's wayvnc 0.9.1 and neatvnc 0.9.0 fail to
start their encoder with FFmpeg 8. Not yet tried: H.264 from a server,
such as wayvnc on a Raspberry Pi, and Fullscreen on All Monitors in the app.

### 16. VNC clipboard

Text in both directions: the original Latin-1 cut text, and the extended
clipboard's UTF-8 text, RTF and HTML where the server has it, fetched when
something pastes. The same switch, off for a new connection, and the same
notice as RDP's.

### 17. VNC connections

- The editor shows what applies to the protocol, with Allow Unencrypted
  for VNC; the connection's row shows its protocol; the search field takes
  `vnc://host[:port]`.
- Import of Remmina's VNC profiles and of TigerVNC's and RealVNC's `.vnc`
  files, and export to `.vnc`.
- Scan Network on ports 5900 to 5903, reading the server's greeting and
  nothing more, so GNOME's prompt never appears.
- Caps Lock and Num Lock kept in step through the QEMU and VMware LED
  pseudo-encodings.
- Decided in this section: sound through QEMU's pseudo-encoding, and
  listening for a server that connects out, as `x11vnc -connect` does.
  UltraVNC's and TightVNC's chat, file transfer and repeater are not
  planned: no Linux server here uses them.

## Remmina parity

What Remmina's RDP plugin offers, and the section that brings it to Mote.

| Remmina | Section |
| --- | --- |
| Profiles, groups, search, quick connect | 2 |
| Passwords in the keyring | 2 |
| `.rdp` import and export | 2 |
| Security negotiation: RDP, TLS, NLA | 3 |
| Certificate checks | 3 |
| Server redirection and load-balance info | 3 |
| Client name, start-up program, console session | 3 |
| Resolution: client, custom, dynamic | 4 |
| Scaling and fullscreen | 4 |
| Clipboard, with the option to turn it off | 5 |
| Codec choice (GFX AVC444, AVC420, RemoteFX); color depth is moot | 6 |
| Quality presets and visual effect flags, which Linux servers ignore | 6 |
| Network type hint, which only xrdp acts on | 6 |
| Sound: here, remote or off (the last two alike on Linux servers); microphone | 7 |
| Shared folders (one per connection; xrdp only) | 8 |
| Printers, smart cards: no Linux server takes them | 8 (not done) |
| Serial and parallel ports, USB: no Linux server takes them | 8 (not done) |
| SSH tunnel | 9 |
| RD Gateway | 9 |
| Wake-on-LAN | 9 |
| Auto-reconnect | 9 |
| Tabs, floating toolbar, scrolled and fullscreen views | 4, 10 |
| Keyboard grab, send Ctrl+Alt+Del | 10 |
| Screenshot, view only | 10 |
| Multiple monitors; span, which Wayland doesn't allow | 10 |
| Pre and post commands | 10 |
| Keyboard layout | 11 |
| Left-handed mouse: GNOME swaps the buttons before Mote sees them | 3 |
