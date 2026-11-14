# Mote

A remote desktop client for GNOME, for Linux machines that run GNOME Remote
Desktop, xrdp, KRdp or a VNC server, and for Macs. Each section below is
what a release brought; the detail is in the commit history.

## What's new in 1.6.0

### Translations:

- Italian by @mrCabez - (#4)

## What's new in 1.5.3

### Fixes:

- About and Settings on macOS went fullscreen over a fullscreen session

## What's new in 1.5.2

### Fixes:

- the session's pointer on macOS was twice its size
- clicks on macOS landed beside the pointer's tip
- fullscreen on macOS hid the sidebar

## What's new in 1.5.1

### Fixes:

- macOS windows showed their shadow before their content
- About opened slowly on macOS

## What's new in 1.5.0

### New features:

- Mote for macOS on Apple silicon
- Automatic updates on macOS
- Sound on macOS
- H.264 video on macOS
- Passwords kept in the macOS Keychain
- macOS shortcuts sent to a session that has the keyboard
- New app icon

### Translations:

- French updated by @frenchy82 - (#3)

## What's new in 1.4.0

### New features:

- VNC (macOS) in the connection editor's protocol list
- Connection Info window from the session menu
- Quit in the session menu
- iShareScreen credited in About

### Fixes:

- the session menu closed by itself during Mac sessions

## What's new in 1.3.0

### New features:

- VNC connections beside RDP
- macOS Screen Sharing support
- Encrypted sessions with Macs
- Automatic choice of the sign-in each macOS version takes
- Encrypted VNC sessions
- Stop before an unencrypted VNC session unless allowed
- VNC desktop resized with the window
- Codec choice for VNC connections
- Two-way clipboard for text with VNC servers and Macs
- Caps Lock and Num Lock kept in step with VNC servers
- VNC profiles imported from Remmina
- VNC connections imported from and exported to .vnc files
- Scan Network lists the RDP and VNC servers on the local network
- Scan Network covers additional subnets
- Adding a scanned server opens the connection editor
- Protocol named on every sidebar row
- Several addresses on one connection
- Addresses tried in order until one connects
- Addresses reorder by dragging
- Alternate full address read from and written to .rdp files
- Delete button in the connection editor
- Projects Mote is built on credited in About
- Beta builds in every package format

### Fixes:

- deleting a connection left its session running
- Flatpak bundles never received updates
- adding a scanned server used its name as the address

### Translations:

- French by @frenchy82 - (#1)

## What's new in 1.2.0

### New features:

- Open a connection fullscreen
- Open a connection in a new window
- Window defaults for new connections in Settings
- Option to hide account avatars in the sidebar
- Option to hide the large account avatar on the connection page

## What's new in 1.1.0

### New features:

- Advice on making GNOME Desktop Sharing resize with the window
- Option to turn off the Desktop Sharing size notice
- Flatpak shares connections and settings with the other installs
- Flatpak shares saved passwords with the other installs
- Notice that the server can read the clipboard on a connection's first session
- Copied files shared only when the connection allows it
- Clipboard sharing off by default for new connections

### Fixes:

- Passwords wiped from memory once used
- Startup warning when session logging can record passwords

## What's new in 1.0.0

### New features:

- GNOME Remote Desktop support for Desktop Sharing and Remote Login
- xrdp and KRdp support
- Fractional scaling supported
- Automatic window resizing
- H.264 decoding through FFmpeg or OpenH264
- H.264 decoder choice in Settings
- Advice on what to install when a server sends no H.264
- Two-way clipboard for text and rich text
- Two-way clipboard for images and files
- Remote desktop sound played on this computer
- Microphone sent to the remote desktop
- Shared folder for the remote desktop
- SSH tunnels
- RD Gateway support
- Wake-on-LAN
- Automatic reconnection when the network drops
- Fullscreen on one monitor or on all of them
- Desktop shortcuts held back while a session has focus
- Keyboard returned to this computer when the pointer leaves the session
- Right Ctrl on its own releases the keyboard and shows the header
- Import from .rdp files and Remmina
- Export to .rdp files
- Connection groups in the sidebar
- Sorting and custom ordering in the sidebar
- Robot avatars for connections
- Flatpak and AppImage builds
- RPM and Debian packages
- Builds for x86_64 and ARM64
