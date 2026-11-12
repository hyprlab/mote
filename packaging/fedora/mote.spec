%global appid co.hyprlab.Mote
# The payload is a binary built outside rpmbuild (tools/build-packages.sh),
# so there is nothing to extract debuginfo from.
%global debug_package %{nil}

Name:           mote
Version:        1.5.2
Release:        1%{?dist}
Summary:        Connect to Linux desktops over RDP and VNC
License:        AGPL-3.0-or-later
URL:            https://github.com/hyprlab/mote
Source0:        %{name}-%{version}-bin.tar

# Libraries the app opens when it needs them, which rpm can't see it link:
# sound and the microphone, Opus sound, H.264 through FFmpeg or OpenH264
# (Fedora's from its Cisco repository), and OpenSSL for the anonymous TLS
# of GNOME's and TigerVNC's VNC.
Recommends:     pulseaudio-libs
Recommends:     opus
Recommends:     libavcodec-free
Recommends:     openh264
Recommends:     openssl-libs
# Passwords are kept through the Secret Service.
Recommends:     gnome-keyring

%description
Mote is a remote desktop client for GNOME, for Linux machines that run an
RDP server: GNOME Remote Desktop, xrdp and KRdp. It keeps connections and
their passwords, follows the screen's scale, shares the clipboard, sound and
a folder, and reaches computers through SSH tunnels and RD Gateways.

%prep
%setup -q -n %{name}-%{version}-bin

%install
install -Dm755 mote %{buildroot}%{_bindir}/mote
# The dnf repository on mote.hyprlab.co, the same file the site serves at
# /rpm/mote.repo: an RPM installed from a download then updates with the
# rest of the system. noreplace keeps a user's own edits (enabled=0).
install -Dm644 mote.repo %{buildroot}%{_sysconfdir}/yum.repos.d/mote.repo
# The beta repository, off: a tester turns it on once (dnf config-manager
# setopt mote-beta.enabled=1). Every package, stable or beta, carries it,
# so the first stable to overtake a beta doesn't remove it.
install -Dm644 mote-beta.repo %{buildroot}%{_sysconfdir}/yum.repos.d/mote-beta.repo
install -Dm644 %{appid}.desktop %{buildroot}%{_datadir}/applications/%{appid}.desktop
install -Dm644 %{appid}.metainfo.xml %{buildroot}%{_datadir}/metainfo/%{appid}.metainfo.xml
for size in 256x256 512x512; do
    install -Dm644 icons/$size/%{appid}.png \
        %{buildroot}%{_datadir}/icons/hicolor/$size/apps/%{appid}.png
done
install -Dm644 icons/scalable/%{appid}.svg \
    %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/%{appid}.svg
install -Dm644 icons/symbolic/%{appid}-symbolic.svg \
    %{buildroot}%{_datadir}/icons/hicolor/symbolic/apps/%{appid}-symbolic.svg
# Message catalogues, staged by tools/build-packages.sh; the binary looks
# under %{_datadir}/locale beside itself (src/i18n.rs).
for mo in locale/*/LC_MESSAGES/mote.mo; do
    [ -e "$mo" ] || continue
    install -Dm644 "$mo" %{buildroot}%{_datadir}/"$mo"
done
# No translations yet is an empty list, not a failure.
%find_lang %{name} || echo '%%defattr(-,root,root,-)' > %{name}.lang

%files -f %{name}.lang
%license LICENSE
%{_bindir}/mote
%config(noreplace) %{_sysconfdir}/yum.repos.d/mote.repo
%config(noreplace) %{_sysconfdir}/yum.repos.d/mote-beta.repo
%{_datadir}/applications/%{appid}.desktop
%{_datadir}/metainfo/%{appid}.metainfo.xml
%{_datadir}/icons/hicolor/*/apps/%{appid}.png
%{_datadir}/icons/hicolor/scalable/apps/%{appid}.svg
%{_datadir}/icons/hicolor/symbolic/apps/%{appid}-symbolic.svg

%changelog
* Thu Nov 12 2026 Hyprlab <hyprlab@proton.me> - 1.5.2-1
- The session's pointer at its own size on macOS, clicking where its tip is
- A fullscreen main window keeps its sidebar on macOS

* Thu Nov 12 2026 Hyprlab <hyprlab@proton.me> - 1.5.1-1
- macOS windows appear already drawn
- About opens faster on macOS

* Wed Nov 11 2026 Hyprlab <hyprlab@proton.me> - 1.5.0-1
- A macOS app that updates itself
- A new app icon
- The French translation brought up to date

* Wed Nov 11 2026 Hyprlab <hyprlab@proton.me> - 1.5.0~beta.1-1
- A macOS app that updates itself
- A new app icon
- The French translation brought up to date

* Sun Nov 08 2026 Hyprlab <hyprlab@proton.me> - 1.4.0-1
- VNC (macOS) in the connection editor's protocol list
- Connection Info window from the session menu
- Quit in the session menu
- iShareScreen credited in About
- The session menu closed by itself during Mac sessions

* Sun Nov 08 2026 Hyprlab <hyprlab@proton.me> - 1.3.0-1
- VNC connections beside RDP
- MacOS Screen Sharing support
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
- Deleting a connection left its session running
- Flatpak bundles never received updates
- Adding a scanned server used its name as the address
- French by @frenchy82

* Thu Nov 05 2026 Hyprlab <hyprlab@proton.me> - 1.3.0~beta.3-1
- Several addresses on one connection
- Addresses tried in order until one connects
- Addresses reorder by dragging
- Alternate full address read from and written to .rdp files

* Wed Nov 04 2026 Hyprlab <hyprlab@proton.me> - 1.3.0~beta.2-1
- Scan Network covers additional subnets
- Flatpak bundles receive updates

* Wed Nov 04 2026 Hyprlab <hyprlab@proton.me> - 1.3.0~beta.1-1
- Scan Network lists the RDP servers on the local network
- Delete button in the connection editor
- Deleting a connection ends its session

* Tue Nov 03 2026 Hyprlab <hyprlab@proton.me> - 1.2.0-1
- Connections that open fullscreen or in a new window
- Switches to hide the account avatars

* Mon Nov 02 2026 Hyprlab <hyprlab@proton.me> - 1.1.0-1
- Clipboard sharing off by default
- Copied files shared only when allowed
- Advice on resizing GNOME Desktop Sharing
- Flatpak data shared with the other installs

* Sun Nov 01 2026 Hyprlab <hyprlab@proton.me> - 1.0.0-1
- First stable release
