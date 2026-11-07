//! Connections from elsewhere: `.rdp` and `.vnc` files, and Remmina's
//! profiles.

pub(crate) mod rdp;
pub(crate) mod remmina;
pub(crate) mod vnc;

use crate::connection::Connection;

/// Add `found` to `existing`, skipping any that reach the same account on
/// the same machine as one already there. Returns how many were added.
pub(crate) fn merge(existing: &mut Vec<Connection>, found: Vec<Connection>) -> usize {
    let mut added = 0;
    for c in found {
        if !existing.iter().any(|e| e.same_target(&c)) {
            existing.push(c);
            added += 1;
        }
    }
    added
}
