//! Arithmetic for RakNet's 24-bit serial numbers.

use std::collections::HashSet;

use crate::error::{RaknetError, Result};

pub(crate) const MASK: u32 = 0x00ff_ffff;
const HALF: u32 = 0x0080_0000;
pub(crate) const RECEIVE_WINDOW: u32 = 65_536;

pub(crate) fn next(value: u32) -> u32 {
    value.wrapping_add(1) & MASK
}

pub(crate) fn distance(value: u32, base: u32) -> u32 {
    value.wrapping_sub(base) & MASK
}

pub(crate) fn newer(value: u32, base: u32) -> bool {
    let delta = distance(value, base);
    delta != 0 && delta < HALF
}

/// Contiguous reliable frames need no allocation. Only holes retain state.
#[derive(Default)]
pub(crate) struct ReliableWindow {
    pub(crate) next: u32,
    pending: HashSet<u32>,
}

impl ReliableWindow {
    pub(crate) fn accept(&mut self, index: u32) -> Result<bool> {
        let delta = distance(index, self.next);
        if delta >= HALF {
            return Ok(false);
        }
        if delta >= RECEIVE_WINDOW {
            return Err(RaknetError::PacketParseError);
        }
        if delta != 0 {
            return Ok(self.pending.insert(index));
        }
        self.next = next(self.next);
        while self.pending.remove(&self.next) {
            self.next = next(self.next);
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serial_order_wraps_at_24_bits() {
        assert_eq!(next(MASK), 0);
        assert!(newer(0, MASK));
        assert!(!newer(MASK, 0));
        assert!(!newer(0, 0));
    }

    #[test]
    fn reliable_window_deduplicates_reordered_and_wrapped_frames() {
        let mut window = ReliableWindow {
            next: MASK - 1,
            ..Default::default()
        };
        assert!(window.accept(0).unwrap());
        assert!(!window.accept(0).unwrap());
        assert!(window.accept(MASK).unwrap());
        assert!(window.accept(MASK - 1).unwrap());
        assert_eq!(window.next, 1);
        assert!(!window.accept(MASK).unwrap());
        assert!(window.pending.is_empty());
        assert!(window.accept(1 + RECEIVE_WINDOW).is_err());
    }
}
