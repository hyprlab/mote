//! The server's pointer, drawn sharp on a Retina screen. GTK on macOS shows
//! a cursor image at its size in pixels as if they were points, so it can
//! only show one at the screen's half resolution (the session's display
//! hands it one of the right size). This makes the NSCursor itself: the
//! server's own pixels, shown at the size in points the desktop is drawn
//! at, which macOS draws from at the screen's full resolution.

use objc2::encode::{Encode, Encoding};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_foundation::NSString;

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::remote::Pointer;

#[repr(C)]
#[derive(Clone, Copy)]
struct Size {
    width: f64,
    height: f64,
}

// SAFETY: laid out as CGSize, which NSSize is.
unsafe impl Encode for Size {
    const ENCODING: Encoding = Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]);
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Point {
    x: f64,
    y: f64,
}

// SAFETY: laid out as CGPoint, which NSPoint is.
unsafe impl Encode for Point {
    const ENCODING: Encoding = Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]);
}

/// NSBitmapFormatAlphaNonpremultiplied: the server's pointer isn't.
const ALPHA_NONPREMULTIPLIED: usize = 1 << 1;

pub(crate) struct Cursor(Retained<AnyObject>);

impl Cursor {
    /// The pointer's RGBA pixels (`width * 4` bytes to a row, not
    /// premultiplied), shown `points` in size with its hotspot at `hotspot`,
    /// both in points.
    pub(crate) fn new(
        rgba: &[u8],
        width: usize,
        height: usize,
        points: (f64, f64),
        hotspot: (f64, f64),
    ) -> Option<Self> {
        if width == 0 || height == 0 || rgba.len() < width * height * 4 {
            return None;
        }
        // SAFETY: plain AppKit calls on the main thread, where GTK runs.
        // The bitmap is allocated by AppKit at the size given, and each of
        // its rows is filled within its own length.
        unsafe {
            let planes: *mut *mut u8 = std::ptr::null_mut();
            let rep: Option<Retained<AnyObject>> = msg_send![
                msg_send![class!(NSBitmapImageRep), alloc],
                initWithBitmapDataPlanes: planes,
                pixelsWide: width as isize,
                pixelsHigh: height as isize,
                bitsPerSample: 8isize,
                samplesPerPixel: 4isize,
                hasAlpha: true,
                isPlanar: false,
                colorSpaceName: &*NSString::from_str("NSDeviceRGBColorSpace"),
                bitmapFormat: ALPHA_NONPREMULTIPLIED,
                bytesPerRow: (width * 4) as isize,
                bitsPerPixel: 32isize
            ];
            let rep = rep?;
            let data: *mut u8 = msg_send![&rep, bitmapData];
            if data.is_null() {
                return None;
            }
            std::ptr::copy_nonoverlapping(rgba.as_ptr(), data, width * height * 4);

            let size = Size {
                width: points.0.max(1.0),
                height: points.1.max(1.0),
            };
            let image: Retained<AnyObject> =
                msg_send![msg_send![class!(NSImage), alloc], initWithSize: size];
            let _: () = msg_send![&image, addRepresentation: &*rep];
            let spot = Point {
                x: hotspot.0.clamp(0.0, size.width - 1.0),
                y: hotspot.1.clamp(0.0, size.height - 1.0),
            };
            let cursor: Option<Retained<AnyObject>> = msg_send![
                msg_send![class!(NSCursor), alloc],
                initWithImage: &*image,
                hotSpot: spot
            ];
            cursor.map(Self)
        }
    }

    /// Make it the pointer's shape, over whatever GTK last set.
    pub(crate) fn set(&self) {
        // SAFETY: as above.
        unsafe {
            let _: () = msg_send![&self.0, set];
        }
    }
}

/// The server's pointer scaled to `size` pixels, for GTK, which shows
/// them as points.
pub(crate) fn shrunk(pointer: &Pointer, size: (i32, i32)) -> gdk::Texture {
    use gtk::gdk_pixbuf::{Colorspace, InterpType, Pixbuf};
    let (w, h) = (i32::from(pointer.width), i32::from(pointer.height));
    let full = Pixbuf::from_bytes(
        &glib::Bytes::from(&pointer.rgba),
        Colorspace::Rgb,
        true,
        8,
        w,
        h,
        w * 4,
    );
    let pixbuf = if size == (w, h) {
        full
    } else {
        full.scale_simple(size.0, size.1, InterpType::Hyper)
            .unwrap_or(full)
    };
    gdk::MemoryTexture::new(
        pixbuf.width(),
        pixbuf.height(),
        gdk::MemoryFormat::R8g8b8a8,
        &pixbuf.read_pixel_bytes(),
        pixbuf.rowstride() as usize,
    )
    .upcast()
}
