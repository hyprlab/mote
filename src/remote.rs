//! What the interface knows of a session, whatever its protocol: the input
//! it sends and the pointer shapes it draws. The engines in `rdp` (and,
//! later, `vnc`) translate between these and their own wire formats.

pub(crate) mod input;

/// A pointer shape from the server.
pub(crate) struct Pointer {
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) hotspot_x: u16,
    pub(crate) hotspot_y: u16,
    /// RGBA, not premultiplied, `width * 4` bytes to a row.
    pub(crate) rgba: Vec<u8>,
}
