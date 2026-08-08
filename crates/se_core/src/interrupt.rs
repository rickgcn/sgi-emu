//! Provides per-CPU interrupt delivery and burst-truncation bits.
//!
//! Each [`InterruptWord`] contains guest interrupt lines in bits 0 through 62
//! and [`EVENT_TRUNCATE`] in bit 63. Clones share one atomic word. Relaxed atomic
//! operations make bit updates race-free but do not synchronize any other
//! machine state; deterministic scheduling and ownership establish that state's
//! ordering separately.

use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Bit reserved for truncating the currently active CPU burst.
pub const EVENT_TRUNCATE: u64 = 1_u64 << 63;

/// Mask containing every guest-visible interrupt-line bit.
pub const GUEST_INTERRUPT_MASK: u64 = EVENT_TRUNCATE - 1;

/// Holds the interrupt and burst-truncation bits observed by one guest CPU.
///
/// Cloning this value creates another handle to the same atomic bitset.
#[derive(Clone, Debug)]
pub struct InterruptWord {
    bits: Arc<AtomicU64>,
}

impl InterruptWord {
    /// Creates a cleared interrupt word.
    #[must_use]
    pub fn new() -> Self {
        Self {
            bits: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Loads all interrupt bits with relaxed ordering.
    ///
    /// This operation observes only the bitset and does not acquire other state.
    #[inline]
    #[must_use]
    pub fn load_relaxed(&self) -> u64 {
        self.bits.load(Ordering::Relaxed)
    }

    /// Atomically sets every bit in `mask` with relaxed ordering.
    #[inline]
    pub fn set_mask(&self, mask: u64) {
        self.bits.fetch_or(mask, Ordering::Relaxed);
    }

    /// Atomically clears every bit in `mask` with relaxed ordering.
    #[inline]
    pub fn clear_mask(&self, mask: u64) {
        self.bits.fetch_and(!mask, Ordering::Relaxed);
    }

    /// Requests that the current CPU burst stop at its next instruction boundary.
    #[inline]
    pub fn request_event_truncate(&self) {
        self.set_mask(EVENT_TRUNCATE);
    }

    /// Clears the burst-local truncation request without changing guest lines.
    #[inline]
    pub fn clear_event_truncate(&self) {
        self.clear_mask(EVENT_TRUNCATE);
    }
}

impl Default for InterruptWord {
    fn default() -> Self {
        Self::new()
    }
}

/// Accepts the asserted level of an interrupt input.
pub trait InterruptSink {
    /// Drives the connected line high or low.
    fn set(&self, level: bool);
}

/// Reports an invalid direct interrupt-line connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterruptLineError {
    /// Guest interrupt lines are limited to bits 0 through 62.
    InvalidLine(u8),
}

impl fmt::Display for InterruptLineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLine(line) => write!(formatter, "interrupt line {line} is outside 0..=62"),
        }
    }
}

impl Error for InterruptLineError {}

/// A sink wired directly to one line in one CPU's interrupt word.
#[derive(Clone, Debug)]
pub struct WordLineSink {
    word: InterruptWord,
    mask: u64,
}

impl WordLineSink {
    /// Connects a guest interrupt line to a CPU interrupt word.
    ///
    /// # Errors
    ///
    /// Returns [`InterruptLineError::InvalidLine`] when `line` is greater than
    /// 62; bit 63 is reserved for [`EVENT_TRUNCATE`].
    pub fn new(word: InterruptWord, line: u8) -> Result<Self, InterruptLineError> {
        if line > 62 {
            return Err(InterruptLineError::InvalidLine(line));
        }
        Ok(Self {
            word,
            mask: 1_u64 << line,
        })
    }

    /// Returns the connected line mask.
    #[must_use]
    pub const fn mask(&self) -> u64 {
        self.mask
    }
}

impl InterruptSink for WordLineSink {
    fn set(&self, level: bool) {
        if level {
            self.word.set_mask(self.mask);
        } else {
            self.word.clear_mask(self.mask);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EVENT_TRUNCATE, GUEST_INTERRUPT_MASK, InterruptLineError, InterruptSink, InterruptWord,
        WordLineSink,
    };

    #[test]
    fn direct_line_changes_only_its_guest_bit() {
        let word = InterruptWord::new();
        let sink = WordLineSink::new(word.clone(), 17).unwrap();
        sink.set(true);
        assert_eq!(word.load_relaxed(), 1_u64 << 17);
        word.request_event_truncate();
        sink.set(false);
        assert_eq!(word.load_relaxed(), EVENT_TRUNCATE);
        word.clear_event_truncate();
        assert_eq!(word.load_relaxed(), 0);
    }

    #[test]
    fn truncate_bit_cannot_be_connected_as_a_guest_line() {
        assert!(matches!(
            WordLineSink::new(InterruptWord::new(), 63),
            Err(InterruptLineError::InvalidLine(63))
        ));
        assert_eq!(GUEST_INTERRUPT_MASK & EVENT_TRUNCATE, 0);
    }
}
