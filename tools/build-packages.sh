#!/usr/bin/env bash
#
# Build the native packages: the RPM for Fedora and the Debian package for
# Debian and Ubuntu.
#
#   tools/build-packages.sh            # builds both
#   tools/build-packages.sh rpm        # builds the RPM
#   tools/build-packages.sh deb        # builds the Debian package
#
# Output lands in packaging/out/:
#   mote-<ver>-1.<dist>.<arch>.rpm     from the host's (Fedora's) cargo build
#   mote_<ver>-1_<arch>.deb            from a Debian 13 cargo build
#                                      (a beta's <ver> is 0.2.0~beta.1 in both)
#
# Each package wraps a release binary built against its own distribution's
# libraries. The RPM's comes from the host. The Debian package's comes from a
# Debian 13 container (podman), because a binary built on Fedora links against
# a newer C library than Debian has; Debian 13 has the libadwaita 1.7 the app
# needs, and what builds there runs on the Ubuntu releases that have it too.
# On a Debian host with the build dependencies, the deb is built in place.
# Each is built for the machine's own architecture: the ARM64 ones come from
# .github/workflows/build-packages.yml, on GitHub's arm64 runners.
set -euo pipefail

APP_ID="co.hyprlab.Mote"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/packaging/out"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
# Neither format can hold a dash there: 0.2.0-beta.1 becomes 0.2.0~beta.1,
# which both sort before 0.2.0.
PKG_VERSION="${VERSION//-/\~}"
WHAT="${1:-all}"
DEB_IMAGE="${MOTE_DEB_IMAGE:-docker.io/library/rust:1-trixie}"

[ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"
# /out is the host's packaging/out when the deb build re-ran this script in
# its container.
[ -n "${MOTE_DEB_CONTAINER:-}" ] && OUT=/out
mkdir -p "$OUT"

# What both packages carry, staged into $1: the release binary, the launcher
# and metainfo with their translated fields merged in, a catalogue for each
# po/<lang>.po, and the icons.
stage_payload() {
    local stage="$1" po lang size
    mkdir -p "$stage/icons/256x256" "$stage/icons/512x512" "$stage/icons/scalable" "$stage/icons/symbolic"
    cp "$ROOT/target/release/mote" "$stage/mote"
    cp "$ROOT/LICENSE" "$stage/LICENSE"
    msgfmt --desktop --template="$ROOT/data/$APP_ID.desktop" -d "$ROOT/po" -o "$stage/$APP_ID.desktop"
    msgfmt --xml --template="$ROOT/data/$APP_ID.metainfo.xml" -d "$ROOT/po" -o "$stage/$APP_ID.metainfo.xml"
    for po in "$ROOT"/po/*.po; do
        [ -e "$po" ] || continue
        lang=$(basename "$po" .po)
        mkdir -p "$stage/locale/$lang/LC_MESSAGES"
        msgfmt -o "$stage/locale/$lang/LC_MESSAGES/mote.mo" "$po"
    done
    for size in 256x256 512x512; do
        cp "$ROOT/data/icons/hicolor/$size/apps/$APP_ID.png" "$stage/icons/$size/$APP_ID.png"
    done
    cp "$ROOT/data/icons/hicolor/scalable/apps/$APP_ID.svg" "$stage/icons/scalable/$APP_ID.svg"
    cp "$ROOT/data/icons/hicolor/symbolic/apps/$APP_ID-symbolic.svg" "$stage/icons/symbolic/$APP_ID-symbolic.svg"
}

build_rpm() {
    # Keep the spec's version in step with Cargo.toml.
    sed -i "s/^Version:.*/Version:        $PKG_VERSION/" "$ROOT/packaging/fedora/mote.spec"

    echo "==> Building the release binary (host)"
    ( cd "$ROOT" && cargo build --release )

    echo "==> Staging the RPM payload"
    local work stage
    work="$(mktemp -d)"
    stage="$work/mote-$PKG_VERSION-bin"
    stage_payload "$stage"
    # The dnf repository on mote.hyprlab.co, so an RPM installed from a
    # download keeps updating with the rest of the system.
    cp "$ROOT/packaging/fedora/mote.repo"      "$stage/mote.repo"
    cp "$ROOT/packaging/fedora/mote-beta.repo" "$stage/mote-beta.repo"
    mkdir -p "$work/rpmbuild/SOURCES"
    tar -C "$work" -cf "$work/rpmbuild/SOURCES/mote-$PKG_VERSION-bin.tar" "mote-$PKG_VERSION-bin"

    echo "==> rpmbuild"
    rpmbuild -bb --define "_topdir $work/rpmbuild" "$ROOT/packaging/fedora/mote.spec"
    cp "$work/rpmbuild/RPMS/$(rpm --eval '%{_arch}')/"mote-*.rpm "$OUT/"
    rm -rf "$work"
    echo "==> RPM done"
}

build_deb() {
    if [ ! -f /etc/debian_version ]; then
        echo "==> Building the Debian package in $DEB_IMAGE (podman)"
        # The tree is copied in so target/ is never shared with the host's
        # builds, named volumes keep the container's target/ and crate
        # downloads between runs, and `label=disable` spares relabelling the
        # working tree for SELinux.
        podman run --rm \
            --security-opt label=disable \
            -e MOTE_DEB_CONTAINER=1 \
            -v "$ROOT":/src:ro \
            -v "$OUT":/out \
            -v mote-deb-target:/build/target \
            -v mote-deb-cargo:/usr/local/cargo/registry \
            "$DEB_IMAGE" \
            sh -euc '
                mkdir -p /build
                tar -C /src --exclude=./target --exclude=./temp --exclude=./.git \
                            --exclude=./build-dir --exclude=./.flatpak-builder \
                            --exclude=./packaging/out --exclude=./AppDir -cf - . \
                    | tar -C /build -xf -
                exec /build/tools/build-packages.sh deb
            '
        return
    fi

    if [ -n "${MOTE_DEB_CONTAINER:-}" ]; then
        echo "==> Installing the build dependencies"
        apt-get update -qq
        DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
            build-essential pkg-config dpkg-dev gettext \
            libgtk-4-dev libadwaita-1-dev >/dev/null
    fi

    echo "==> Building the release binary ($(. /etc/os-release && echo "$PRETTY_NAME"))"
    ( cd "$ROOT" && cargo build --release )

    echo "==> Staging the Debian package"
    local work stage pkg arch deps mo
    work="$(mktemp -d)"
    stage="$work/payload"
    pkg="$work/mote"
    arch="$(dpkg --print-architecture)"
    stage_payload "$stage"
    install -Dm755 "$stage/mote" "$pkg/usr/bin/mote"
    install -Dm644 "$stage/$APP_ID.desktop" "$pkg/usr/share/applications/$APP_ID.desktop"
    install -Dm644 "$stage/$APP_ID.metainfo.xml" "$pkg/usr/share/metainfo/$APP_ID.metainfo.xml"
    for size in 256x256 512x512 scalable symbolic; do
        for f in "$stage/icons/$size"/*; do
            install -Dm644 "$f" "$pkg/usr/share/icons/hicolor/$size/apps/$(basename "$f")"
        done
    done
    # The binary looks for its catalogues under share/locale beside itself
    # (src/i18n.rs).
    for mo in "$stage"/locale/*/LC_MESSAGES/mote.mo; do
        [ -e "$mo" ] || continue
        install -Dm644 "$mo" "$pkg/usr/share/${mo#"$stage"/}"
    done
    # The apt repository on mote.hyprlab.co, the same files the site serves
    # at /deb/mote.sources and /deb/mote-beta.sources (key included), so a
    # package installed from a download keeps updating with the rest of the
    # system. The beta's is off, as in the RPM, and every package carries
    # it. Both are conffiles: a user's edits are kept on upgrade.
    install -Dm644 "$ROOT/packaging/debian/mote.sources" "$pkg/etc/apt/sources.list.d/mote.sources"
    install -Dm644 "$ROOT/packaging/debian/mote-beta.sources" "$pkg/etc/apt/sources.list.d/mote-beta.sources"
    mkdir -p "$pkg/DEBIAN" "$pkg/usr/share/doc/mote"
    printf '/etc/apt/sources.list.d/mote.sources\n/etc/apt/sources.list.d/mote-beta.sources\n' > "$pkg/DEBIAN/conffiles"
    {
        printf 'Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/\n'
        printf 'Upstream-Name: Mote\nSource: https://github.com/hyprlab/mote\n\n'
        printf 'Files: *\nCopyright: Hyprlab <hyprlab@proton.me>\nLicense: AGPL-3.0-or-later\n'
        sed 's/^[[:space:]]*$/./; s/^/ /' "$stage/LICENSE"
    } > "$pkg/usr/share/doc/mote/copyright"

    # dpkg-shlibdeps turns the libraries the binary links into package names
    # and minimum versions; it wants a source control file to exist.
    mkdir -p "$work/debian"
    printf 'Source: mote\n\nPackage: mote\nArchitecture: any\n' > "$work/debian/control"
    deps="$(cd "$work" && dpkg-shlibdeps -O -e"$pkg/usr/bin/mote" 2>/dev/null)"
    deps="${deps#shlibs:Depends=}"
    [ -n "$deps" ] || { echo "dpkg-shlibdeps found no dependencies" >&2; exit 1; }
    # The app's icons are SVG, which GTK reads through a loader no linked
    # library names: Debian's GTK only recommends it.
    deps="$deps, librsvg2-common | glycin-loaders"
    # The libraries Mote opens itself, which dpkg-shlibdeps can't see: sound
    # and the microphone, Opus, and H.264 through FFmpeg or OpenH264.
    cat > "$pkg/DEBIAN/control" <<EOF
Package: mote
Version: $PKG_VERSION-1
Section: net
Priority: optional
Architecture: $arch
Maintainer: Hyprlab <hyprlab@proton.me>
Installed-Size: $(du -sk --exclude=DEBIAN "$pkg" | cut -f1)
Depends: $deps
Recommends: libpulse0, libopus0, libavcodec62 | libavcodec61, libopenh264-8, libssl3t64 | libssl3, gnome-keyring
Homepage: https://github.com/hyprlab/mote
Description: Connect to Linux desktops over RDP
 Mote is a remote desktop client for GNOME, for Linux machines that run an
 RDP server: GNOME Remote Desktop, xrdp and KRdp. It keeps connections and
 their passwords, follows the screen's scale, shares the clipboard, sound
 and a folder, and reaches computers through SSH tunnels and RD Gateways.
EOF

    echo "==> dpkg-deb"
    dpkg-deb --build --root-owner-group "$pkg" "$OUT/mote_${PKG_VERSION}-1_${arch}.deb"
    rm -rf "$work"
    echo "==> DEB done"
}

case "$WHAT" in
    rpm) build_rpm ;;
    deb) build_deb ;;
    all) build_rpm; build_deb ;;
    *)   echo "usage: $0 [rpm|deb|all]" >&2; exit 1 ;;
esac

[ -n "${MOTE_DEB_CONTAINER:-}" ] && exit 0
echo "==> Packages in $OUT:"
ls -1 "$OUT"
