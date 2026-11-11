#!/usr/bin/env python3
"""Regenerate the app icons from their masters in data/icons/src.

default.svg is the stable app's, default.Devel.svg the same with the beta's
hazard stripe, GNOME's styling for a development build. Each master is drawn
on GNOME's 128 pixel app icon grid. It becomes the scalable hicolor icon and
256 and 512 pixel PNGs beside it for anything that wants a bitmap, and the
stable one the README artwork, data/repo/logo.png. macos.svg is the macOS
app's, drawn on Apple's icon grid instead; it becomes the 1024 pixel PNG in
data/icons/macos that tools/build-macos-app.sh turns into the bundle's
icon, and the macOS build embeds it for About. The PNGs are
rendered with librsvg, the renderer GNOME uses, so they match what the
desktop draws from the SVG.

The symbolic icon (data/icons/hicolor/symbolic/apps) is drawn by hand, not
generated, so it is left alone.

Requires python3-gobject with the Rsvg typelib, and pycairo.
"""
import pathlib, shutil, sys

import cairo, gi
gi.require_version("Rsvg", "2.0")
from gi.repository import Rsvg

ROOT = pathlib.Path(__file__).resolve().parent.parent
SOURCES = ROOT / "data/icons/src"
HICOLOR = ROOT / "data/icons/hicolor"
SHIPPED = {"default": "co.hyprlab.Mote", "default.Devel": "co.hyprlab.Mote.Beta"}


def render(master: pathlib.Path, png: pathlib.Path, size: int) -> None:
    png.parent.mkdir(parents=True, exist_ok=True)
    handle = Rsvg.Handle.new_from_file(str(master))
    surface = cairo.ImageSurface(cairo.FORMAT_ARGB32, size, size)
    rect = Rsvg.Rectangle()
    rect.x, rect.y, rect.width, rect.height = 0, 0, size, size
    if not handle.render_document(cairo.Context(surface), rect):
        sys.exit(f"librsvg could not render {master}")
    surface.write_to_png(str(png))


for source, app_id in SHIPPED.items():
    master = SOURCES / f"{source}.svg"
    scalable = HICOLOR / f"scalable/apps/{app_id}.svg"
    scalable.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(master, scalable)
    for size in (256, 512):
        render(master, HICOLOR / f"{size}x{size}/apps/{app_id}.png", size)
render(SOURCES / "default.svg", ROOT / "data/repo/logo.png", 512)
render(SOURCES / "macos.svg", ROOT / "data/icons/macos/co.hyprlab.Mote.png", 1024)
