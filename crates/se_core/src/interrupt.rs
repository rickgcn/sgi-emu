//! Provides per-CPU guest-interrupt and execution-control delivery.
//!
//! Each [`InterruptWord`] contains guest interrupt lines in bits 0 through 61,
//! [`HOST_WAKE`] in bit 62, and [`EVENT_TRUNCATE`] in bit 63. A
//! [`HostWakeHandle`] lets a host worker request the CPU slow path without access
//! to the deterministic event queue.
//!
//! All bit operations use relaxed ordering. The control bits are coalescing
//! doorbells, not memory-publication primitives; event ownership and a separately
//! synchronized host channel establish the ordering of associated state.

use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Bit reserved for truncating the currently active CPU burst.
pub const EVENT_TRUNCATE: u64 = 1_u64 << 63;

/// Bit reserved for pending host-control work.
pub const HOST_WAKE: u64 = 1_u64 << 62;

/// Mask containing every guest-visible interrupt-line bit.
pub const GUEST_INTERRUPT_MASK: u64 = HOST_WAKE - 1;

/// Holds the guest-interrupt and execution-control bits observed by one CPU.
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
    ///
    /// Callers assigning guest interrupt lines exclude [`HOST_WAKE`] and
    /// [`EVENT_TRUNCATE`], whose lifecycles belong to their dedicated APIs.
    #[inline]
    pub fn set_mask(&self, mask: u64) {
        self.bits.fetch_or(mask, Ordering::Relaxed);
    }

    /// Atomically clears every bit in `mask` with relaxed ordering.
    ///
    /// Callers assigning guest interrupt lines exclude [`HOST_WAKE`] and
    /// [`EVENT_TRUNCATE`], whose lifecycles belong to their dedicated APIs.
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

    /// Creates a host-worker handle for this CPU word.
    #[must_use]
    pub fn host_wake_handle(&self) -> HostWakeHandle {
        HostWakeHandle {
            bits: Arc::clone(&self.bits),
        }
    }

    /// Consumes a pending host wake and reports whether one was present.
    ///
    /// The CPU calls this on its slow path before the runtime drains the host
    /// channel. A concurrent request after this atomic operation remains set for
    /// the current or next drain. This operation does not acquire channel data.
    #[inline]
    #[must_use]
    pub fn take_host_wake(&self) -> bool {
        self.bits.fetch_and(!HOST_WAKE, Ordering::Relaxed) & HOST_WAKE != 0
    }
}

impl Default for InterruptWord {
    fn default() -> Self {
        Self::new()
    }
}

/// Signals pending host-control work to one CPU interrupt word.
///
/// The bit is a coalescing doorbell: repeated requests before consumption remain
/// one request. Associated commands or ingress data must be published through a
/// separately synchronized channel before [`Self::request`] is called. Signaling
/// the bit does not wake a parked host execution thread.
#[derive(Clone, Debug)]
pub struct HostWakeHandle {
    bits: Arc<AtomicU64>,
}

impl HostWakeHandle {
    /// Requests a host-control exit with relaxed ordering.
    #[inline]
    pub fn request(&self) {
        self.bits.fetch_or(HOST_WAKE, Ordering::Relaxed);
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
    /// Guest interrupt lines are limited to bits 0 through 61.
    InvalidLine(u8),
}

impl fmt::Display for InterruptLineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLine(line) => write!(formatter, "interrupt line {line} is outside 0..=61"),
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
    /// 61; bits 62 and 63 are reserved for [`HOST_WAKE`] and
    /// [`EVENT_TRUNCATE`], respectively.
    pub fn new(word: InterruptWord, line: u8) -> Result<Self, InterruptLineError> {
        if line > 61 {
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
    use std::sync::{Arc, Barrier};
    use std::thread;

    use super::{
        EVENT_TRUNCATE, GUEST_INTERRUPT_MASK, HOST_WAKE, InterruptLineError, InterruptSink,
        InterruptWord, WordLineSink,
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
    fn control_bits_cannot_be_connected_as_guest_lines() {
        assert!(WordLineSink::new(InterruptWord::new(), 61).is_ok());
        assert!(matches!(
            WordLineSink::new(InterruptWord::new(), 62),
            Err(InterruptLineError::InvalidLine(62))
        ));
        assert!(matches!(
            WordLineSink::new(InterruptWord::new(), 63),
            Err(InterruptLineError::InvalidLine(63))
        ));
        assert_eq!(GUEST_INTERRUPT_MASK & HOST_WAKE, 0);
        assert_eq!(GUEST_INTERRUPT_MASK & EVENT_TRUNCATE, 0);
    }

    #[test]
    fn host_wake_is_coalesced_and_consumed_independently() {
        let word = InterruptWord::new();
        let wake = word.host_wake_handle();
        word.set_mask(1_u64 << 17);
        word.request_event_truncate();

        wake.request();
        wake.request();
        assert_eq!(
            word.load_relaxed(),
            (1_u64 << 17) | HOST_WAKE | EVENT_TRUNCATE
        );
        assert!(word.take_host_wake());
        assert!(!word.take_host_wake());
        assert_eq!(word.load_relaxed(), (1_u64 << 17) | EVENT_TRUNCATE);
    }

    #[test]
    fn host_wake_can_be_reasserted_after_slow_path_consumption() {
        let word = InterruptWord::new();
        let wake = word.host_wake_handle();
        let barrier = Arc::new(Barrier::new(2));
        let worker_barrier = Arc::clone(&barrier);

        wake.request();
        let worker = thread::spawn(move || {
            worker_barrier.wait();
            wake.request();
        });

        assert!(word.take_host_wake());
        barrier.wait();
        worker.join().unwrap();
        assert_eq!(word.load_relaxed() & HOST_WAKE, HOST_WAKE);
    }
}
