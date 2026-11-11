#!/usr/bin/env python3
"""Make Mote.app carry everything it runs on, so a Mac without Homebrew
runs it. Called by tools/build-macos-app.sh, on a Mac, once the binary and
the icon are in the bundle:

    tools/macos-bundle.py <Mote.app>

- Every library the binary loads from Homebrew, and every library those
  load, is copied into Contents/Frameworks and told to find the others
  there (@rpath), so nothing points into Homebrew any more. The build fails
  if anything still does.
- GTK's data comes along under Contents/Resources: the GSettings schemas it
  reads, the Adwaita icons, the translations (Mote's own, and GTK's and
  libadwaita's for the same languages), the SVG loader that draws the app's
  colored icons, with its list of loaders, and a fontconfig setup. The app
  points GTK at them when it starts (src/macos/bundle.rs).
- LSMinimumSystemVersion is the newest macOS any bundled library was built
  for: Homebrew builds for the macOS it runs on, so a bundle made on macOS
  15 runs on 15 and later.

Written for the python3 that comes with the Command Line Tools (3.9).
"""
import os
import plistlib
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BREW = Path(subprocess.run(["brew", "--prefix"], capture_output=True, text=True,
                           check=True).stdout.strip())
# What the bundle loads from outside itself: the system's own libraries.
SYSTEM = ("/usr/lib/", "/System/")


def otool(*args):
    return subprocess.run(["otool", *args], capture_output=True, text=True, check=True).stdout


def install_name_tool(*args):
    subprocess.run(["install_name_tool", *args], check=True, stderr=subprocess.DEVNULL)


def dependencies(binary):
    """The libraries a Mach-O file names, as written in it."""
    lines = otool("-L", str(binary)).splitlines()[1:]
    names = [line.strip().split(" (compatibility")[0] for line in lines]
    own = otool("-D", str(binary)).splitlines()[1:]
    return [n for n in names if n not in own]


def rpaths(binary):
    out = otool("-l", str(binary)).splitlines()
    return [out[i + 2].split()[1] for i, line in enumerate(out)
            if line.strip() == "cmd LC_RPATH"]


def resolve(name, referrer):
    """Where a library named in `referrer` really is, or None if it is the
    system's."""
    if name.startswith(SYSTEM):
        return None
    origin = Path(os.path.realpath(referrer)).parent
    if name.startswith("@loader_path/"):
        return Path(os.path.realpath(origin / name[len("@loader_path/"):]))
    if name.startswith("@rpath/"):
        for rpath in rpaths(referrer):
            base = Path(rpath.replace("@loader_path", str(origin)))
            candidate = base / name[len("@rpath/"):]
            if candidate.exists():
                return Path(os.path.realpath(candidate))
        candidate = BREW / "lib" / name[len("@rpath/"):]
        if candidate.exists():
            return Path(os.path.realpath(candidate))
        sys.exit(f"{referrer} needs {name}, which isn't found")
    return Path(os.path.realpath(name))


def bundle_libraries(roots, frameworks):
    """Copy the libraries `roots` need, and theirs, into `frameworks`, and
    point every name at the copies."""
    frameworks.mkdir(parents=True, exist_ok=True)
    copied = {}  # real path -> name in Frameworks
    renames = {}  # file in the bundle -> [(name it gives, real path)]
    # Each copy is read as its original, whose @loader_path and rpaths
    # still point where the libraries it names are.
    queue = [(root, origin) for root, origin in roots]
    while queue:
        binary, origin = queue.pop()
        renames[binary] = []
        for name in dependencies(origin):
            real = resolve(name, origin)
            if real is None:
                continue
            renames[binary].append((name, real))
            if real in copied:
                continue
            leaf = real.name
            if leaf in copied.values():
                sys.exit(f"two libraries are called {leaf}")
            copied[real] = leaf
            target = frameworks / leaf
            shutil.copyfile(real, target)
            target.chmod(0o755)
            queue.append((target, real))
    for binary, names in renames.items():
        changes = []
        for name, real in names:
            changes += ["-change", name, f"@rpath/{copied[real]}"]
        if binary.parent == frameworks:
            changes += ["-id", f"@rpath/{binary.name}"]
        if changes:
            install_name_tool(*changes, str(binary))
        for rpath in rpaths(binary):
            install_name_tool("-delete_rpath", rpath, str(binary))
        here = os.path.relpath(frameworks, binary.parent)
        install_name_tool("-add_rpath", "@loader_path" if here == "." else f"@loader_path/{here}",
                          str(binary))
        # Changing a library voids its signature, and Apple silicon won't
        # load unsigned code; an ad hoc one holds until the bundle is signed.
        subprocess.run(["codesign", "--force", "--sign", "-", str(binary)], check=True,
                       capture_output=True)
    return [frameworks / leaf for leaf in copied.values()]


def check_self_contained(binaries):
    for binary in binaries:
        for name in dependencies(binary) + rpaths(binary):
            if name.startswith(str(BREW)) or name.startswith("/usr/local/"):
                sys.exit(f"{binary} still loads {name}")


def minimum_macos(binaries):
    newest = (0,)
    for binary in binaries:
        for match in re.finditer(r"minos (\d+(?:\.\d+)*)", otool("-l", str(binary))):
            newest = max(newest, tuple(int(p) for p in match.group(1).split(".")))
    return ".".join(str(p) for p in (newest + (0,))[:max(2, len(newest))])


def copy_data(resources, languages):
    share = resources / "share"
    # GSettings schemas: GTK's own, which its file and color choosers read.
    schemas = share / "glib-2.0/schemas"
    schemas.mkdir(parents=True)
    for xml in (BREW / "share/glib-2.0/schemas").glob("org.gtk.gtk4.*.gschema.xml"):
        shutil.copyfile(xml, schemas / xml.name)
    subprocess.run(["glib-compile-schemas", str(schemas)], check=True)
    # Icons: Adwaita without its cursors, which macOS draws itself, and
    # hicolor's index, which the app icon's theme falls back to.
    adwaita = share / "icons/Adwaita"
    adwaita.mkdir(parents=True)
    source = BREW / "share/icons/Adwaita"
    for entry in ("index.theme", "scalable", "symbolic", "16x16"):
        if (source / entry).is_dir():
            shutil.copytree(source / entry, adwaita / entry, symlinks=False)
        elif (source / entry).exists():
            shutil.copyfile(source / entry, adwaita / entry)
    (share / "icons/hicolor").mkdir(parents=True)
    shutil.copyfile(BREW / "share/icons/hicolor/index.theme", share / "icons/hicolor/index.theme")
    # Translations: Mote's, compiled here, and GTK's and libadwaita's.
    for lang in languages:
        messages = share / "locale" / lang / "LC_MESSAGES"
        messages.mkdir(parents=True)
        subprocess.run(["msgfmt", "--check", "-o", str(messages / "mote.mo"),
                        str(ROOT / "po" / f"{lang}.po")], check=True)
        for domain in ("gtk40", "libadwaita", "glib20"):
            mo = BREW / "share/locale" / lang / "LC_MESSAGES" / f"{domain}.mo"
            if mo.exists():
                shutil.copyfile(mo, messages / mo.name)
    # Fonts: pango draws with Core Text, but GTK starts fontconfig too, which
    # wants a configuration; this one reads macOS's fonts and caches with the
    # app's other caches.
    fonts = resources / "etc/fonts"
    fonts.mkdir(parents=True)
    (fonts / "fonts.conf").write_text("""<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">
<fontconfig>
  <dir>/System/Library/Fonts</dir>
  <dir>/Library/Fonts</dir>
  <dir>~/Library/Fonts</dir>
  <cachedir>~/Library/Caches/co.hyprlab.Mote/fontconfig</cachedir>
</fontconfig>
""")


def svg_loader(contents):
    """librsvg's loader for gdk-pixbuf, which draws the colored SVG icons,
    in PlugIns, with the list gdk-pixbuf reads its loaders from. The list
    needs whole paths, so it is kept with @CONTENTS@ in their place, which
    the app fills in when it starts."""
    plugins = contents / "PlugIns/gdk-pixbuf"
    plugins.mkdir(parents=True)
    loader = plugins / "libpixbufloader_svg.so"
    origin = Path(os.path.realpath(BREW / "lib/gdk-pixbuf-2.0/2.10.0/loaders/libpixbufloader_svg.so"))
    shutil.copyfile(origin, loader)
    loader.chmod(0o755)
    return loader, origin


def write_loader_cache(contents, loader):
    query = subprocess.run([str(BREW / "bin/gdk-pixbuf-query-loaders"), str(loader)],
                           capture_output=True, text=True, check=True).stdout
    cache = contents / "Resources/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache"
    cache.parent.mkdir(parents=True)
    lines = [line for line in query.splitlines() if not line.startswith("#")]
    cache.write_text("\n".join(lines).replace(str(contents), "@CONTENTS@") + "\n")


def main():
    app = Path(sys.argv[1]).resolve()
    contents = app / "Contents"
    executable = contents / "MacOS" / plistlib.loads((contents / "Info.plist").read_bytes())[
        "CFBundleExecutable"]
    languages = sorted(p.stem for p in (ROOT / "po").glob("*.po"))
    copy_data(contents / "Resources", languages)
    loader, origin = svg_loader(contents)
    libraries = bundle_libraries([(executable, executable), (loader, origin)], contents / "Frameworks")
    write_loader_cache(contents, loader)
    everything = [executable, loader] + libraries
    check_self_contained(everything)
    plist_path = contents / "Info.plist"
    plist = plistlib.loads(plist_path.read_bytes())
    plist["LSMinimumSystemVersion"] = minimum_macos(everything)
    plist_path.write_bytes(plistlib.dumps(plist))
    print(f"{len(libraries)} libraries, macOS {plist['LSMinimumSystemVersion']} or later")


if __name__ == "__main__":
    main()
