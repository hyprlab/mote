# Installing

The short version is in the [README](../README.md#installing); this page has
the rest. Every Linux format runs on **x86_64 and aarch64 (ARM64)**, and they
all keep connections, settings and passwords in the same place, so moving from
one to another loses nothing. The [macOS app](#macos) is for Macs with Apple
silicon.

## Flatpak (recommended)

Works on any distribution. Installing from the signed repository picks the
right architecture on its own and keeps the app updated:

```sh
flatpak install --user --from https://mote.hyprlab.co/flatpak/co.hyprlab.Mote.flatpakref
```

**Prefer a direct download?** Each release carries `Mote-x86_64.flatpak` and
`Mote-aarch64.flatpak`; take the one matching `uname -m` from the
[latest release](https://github.com/hyprlab/mote/releases/latest) and run
`flatpak install --user ./Mote-*.flatpak`. The bundle carries the repository's
address and signing key, so it still receives updates from it. (A bundle holds
a single architecture; the repository holds both.) Bundles downloaded before
2026-11-04 don't: install from the repository above to get updates.

**Bazaar** does not install from a `.flatpakref` whose repository it does not
already know. Add the repository once, and Mote then shows up in Bazaar's
search:

```sh
flatpak remote-add --user --if-not-exists mote https://mote.hyprlab.co/flatpak/mote.flatpakrepo
```

## Fedora

Add the signed dnf repository on mote.hyprlab.co and install from it. Mote then
updates with the rest of the system, through GNOME Software or `dnf upgrade`:

```sh
sudo curl -fsSLo /etc/yum.repos.d/mote.repo https://mote.hyprlab.co/rpm/mote.repo
sudo dnf install mote
```

dnf asks once to trust the repository's signing key, fingerprint
`EE0F 68DF 6284 D4CC 2FA2 CBAC FF89 3368 68D9 1169`.

The `.rpm` is also attached to each
[release](https://github.com/hyprlab/mote/releases/latest). The package adds
the repository itself, so an RPM installed from a download keeps updating too;
it is the same `/etc/yum.repos.d/mote.repo`, kept if you edit it (set
`enabled=0` to stop updates from it).

The RPM is built for Fedora 44. On anything older, use the Flatpak or
[build from source](BUILDING.md).

## Debian and Ubuntu

Add the signed apt repository on mote.hyprlab.co and install from it. Mote then
updates with the rest of the system, through GNOME Software or `apt upgrade`:

```sh
sudo curl -fsSLo /etc/apt/sources.list.d/mote.sources https://mote.hyprlab.co/deb/mote.sources
sudo apt update
sudo apt install mote
```

The `.sources` file carries the repository's signing key, the same key as the
dnf repository's, so there is no key to import separately.

The `.deb` is also attached to each
[release](https://github.com/hyprlab/mote/releases/latest); install it with
`sudo apt install ./mote_*_amd64.deb` (`_arm64.deb` on ARM), which fetches its
dependencies. The package adds the repository itself, as the RPM does: the same
`/etc/apt/sources.list.d/mote.sources`, kept if you edit it (add a line
`Enabled: no` to stop updates from it).

The package needs libadwaita 1.7: Debian 13 (trixie) and later, or Ubuntu 25.04
and later. Ubuntu 24.04 LTS and the distributions built on it, such as Linux
Mint 22, have an older libadwaita; use the Flatpak there.

## AppImage

One file that runs on any distribution. Download the one matching `uname -m`,
make it executable and run it:

```sh
curl -fLO https://mote.hyprlab.co/download/Mote-x86_64.AppImage
chmod +x Mote-x86_64.AppImage
./Mote-x86_64.AppImage
```

On ARM, the file is `Mote-aarch64.AppImage`. Each release carries both as well.

Each AppImage knows where its updates come from: add it to
[Gear Lever](https://flathub.org/apps/it.mijorus.gearlever) or
AppImageLauncher, or run `appimageupdatetool` on it, and it fetches only the
parts that changed.

## macOS

Mote for macOS runs on Macs with Apple silicon, on macOS 15 or later. Download
[Mote.dmg](https://mote.hyprlab.co/download/Mote.dmg), which each release
carries too, open it and drag Mote to Applications.

Mote is signed with its own certificate rather than through Apple, so the
first time it opens, macOS says Apple could not verify that it is free of
malware, and offers only to move it to the Trash. To open it anyway, once:

1. Open Mote from Applications, and choose Done.
2. Open System Settings > Privacy & Security, and under Security choose Open
   Anyway beside the line about Mote.
3. Confirm with your password or Touch ID, then choose Open.

From then on it opens like any other app, and updates itself: at its second
launch it asks whether to check for updates on its own, and Mote > Check for
Updates checks at once. An update installs without macOS asking again.
Passwords are kept in the Keychain.

## Beta channel

Betas come in every format the stable release does. The site's
[beta section](https://mote.hyprlab.co/#beta) has them all, and each beta is on
the [releases page](https://github.com/hyprlab/mote/releases) too. **Beta
builds may be buggy and unstable.**

### Flatpak beta

The Flatpak beta installs beside the stable app as a separate application,
"Mote (beta)" (`co.hyprlab.Mote.Beta`):

```sh
flatpak install --user --from https://mote.hyprlab.co/flatpak/co.hyprlab.Mote-beta.flatpakref
```

Each beta release also carries `Mote-Beta-x86_64.flatpak` and
`Mote-Beta-aarch64.flatpak`. To remove the beta, run
`flatpak uninstall co.hyprlab.Mote.Beta`; the stable app is untouched.

### Fedora beta

Betas come as RPMs from a beta dnf repository. They replace the installed
`mote` package rather than sitting beside it, and share its connections and
settings. If Mote isn't installed yet, add the repository and install:

```sh
sudo curl -fsSLo /etc/yum.repos.d/mote-beta.repo https://mote.hyprlab.co/rpm/mote-beta.repo
sudo dnf install mote
```

If it is, every RPM carries the beta repository, switched off. Turn it on once:

```sh
sudo dnf config-manager setopt mote-beta.enabled=1
sudo dnf upgrade --refresh mote
```

A beta version such as `1.3.0~beta.1` sorts before `1.3.0`, so the stable
release replaces the last beta when it comes out, and the next beta follows it.
To leave the betas, set `mote-beta.enabled=0`; dnf won't move back to an older
version by itself, so run `sudo dnf distro-sync mote` to return to the newest
stable. An RPM installed from a download without turning the repository on
updates from the stable repository only.

### Debian and Ubuntu beta

Betas come from the apt repository's beta suite in the same way. If Mote isn't
installed yet, add the suite and install:

```sh
sudo curl -fsSLo /etc/apt/sources.list.d/mote-beta.sources https://mote.hyprlab.co/deb/mote-beta.sources
sudo apt update && sudo apt install -o Dpkg::Options::=--force-confold mote
```

If it is, every Debian package carries the suite, switched off. Turn it on once:

```sh
sudo sed -i 's/^Enabled: no/Enabled: yes/' /etc/apt/sources.list.d/mote-beta.sources
sudo apt update && sudo apt install mote
```

To leave the betas, set `Enabled: no` again, then run
`sudo apt update && sudo apt install mote/stable` to return to the newest
stable; apt asks before it moves to an older version.

### AppImage beta

The beta AppImage is the beta app, "Mote (beta)" with its own launcher, and
updates from the beta channel:

```sh
curl -fLO https://mote.hyprlab.co/download/beta/Mote-Beta-x86_64.AppImage
chmod +x Mote-Beta-x86_64.AppImage
```

On ARM, the file is `Mote-Beta-aarch64.AppImage`. To leave the betas, delete it
and use the stable AppImage.

### macOS beta

The beta is its own app, "Mote (beta)", which installs beside Mote and
updates from the beta channel, moving on to each stable release as well.
Download [Mote-Beta.dmg](https://mote.hyprlab.co/download/beta/Mote-Beta.dmg)
and open it once as above. To leave the betas, move it to the Trash.

## Runtime requirements

On Linux, a Secret Service provider, such as gnome-keyring, which GNOME
installs, stores passwords. The Flatpak carries everything else. Outside it, Mote loads
[H.264 decoders](BUILDING.md#h264) and [sound libraries](BUILDING.md#sound) when
it runs, and the packages recommend them. The macOS app carries everything it
needs.
