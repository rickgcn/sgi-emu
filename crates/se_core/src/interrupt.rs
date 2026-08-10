//! Provides per-CPU guest-interrupt and execution-control delivery.
//!
//! Each [`InterruptWord`] contains guest interrupt lines in bits 0 through 61,
//! [`HOST_WAKE`] in bit 62, and [`EVENT_TRUNCATE`] in bit 63. A
//! [`HostWakeHandle`] lets a host worker request the CPU slow path without access
//! to the deterministic event queue.
//!
//! Guest-line and event-truncation operations use relaxed ordering. Host workers
//! publish work before a release-ordered request, and the CPU slow path consumes
//! that request with acquire ordering before reading the work. The initial CPU
//! poll remains relaxed and does not acquire host state.
//!
//! Guest interrupt bits represent input levels. The CPU observes those bits but
//! does not clear them when accepting an architectural interrupt; the device or
//! interrupt controller that owns a line drives it low through [`InterruptSink`].
//! Mutation authority is split by API: [`WordLineSink`] can change one validated
//! guest line, [`HostWakeHandle`] can only request [`HOST_WAKE`], and the event
//! scheduler alone controls [`EVENT_TRUNCATE`].
//!
//! A direct [`WordLineSink`] carries one already aggregated level from exactly one
//! logical driver. Multiple interrupt sources first enter a controller through
//! distinct input sinks; the controller alone drives its direct output line.

use std::error::Error;
use std::fmt;
use std::marker::PhantomData;
use std::rc::Rc;
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
/// Cloning this value creates another deterministic-thread handle to the same
/// atomic bitset. The word is neither [`Send`] nor [`Sync`], so guest interrupt
/// lines and event truncation cannot be driven directly by host workers. Use
/// [`Self::host_wake_handle`] to create the thread-safe host-control capability.
///
/// ```compile_fail
/// use se_core::interrupt::InterruptWord;
///
/// fn require_send<T: Send>() {}
///
/// require_send::<InterruptWord>();
/// ```
///
/// ```compile_fail
/// use se_core::interrupt::InterruptWord;
///
/// fn require_sync<T: Sync>() {}
///
/// require_sync::<InterruptWord>();
/// ```
#[derive(Clone, Debug)]
pub struct InterruptWord {
    // Keep every mutation as an atomic read-modify-write so unrelated bit changes
    // preserve a HOST_WAKE release sequence until the slow path consumes it.
    bits: Arc<AtomicU64>,
    single_thread: PhantomData<Rc<()>>,
}

impl InterruptWord {
    /// Creates a cleared interrupt word.
    #[must_use]
    pub fn new() -> Self {
        Self {
            bits: Arc::new(AtomicU64::new(0)),
            single_thread: PhantomData,
        }
    }

    /// Loads all interrupt bits with relaxed ordering.
    ///
    /// This operation observes only the bitset and does not acquire other state.
    /// After observing [`HOST_WAKE`], the slow path calls
    /// [`Self::take_host_wake`] before reading host work.
    #[inline]
    #[must_use]
    pub fn load_relaxed(&self) -> u64 {
        self.bits.load(Ordering::Relaxed)
    }

    #[inline]
    fn set_mask(&self, mask: u64) {
        self.bits.fetch_or(mask, Ordering::Relaxed);
    }

    #[inline]
    fn clear_mask(&self, mask: u64) {
        self.bits.fetch_and(!mask, Ordering::Relaxed);
    }

    /// Requests that the current CPU burst stop at its next CPU safe point.
    #[inline]
    pub(crate) fn request_event_truncate(&self) {
        self.set_mask(EVENT_TRUNCATE);
    }

    /// Clears the burst-local truncation request without changing guest lines.
    #[inline]
    pub(crate) fn clear_event_truncate(&self) {
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
    /// channel. When it consumes a release-ordered request, subsequent reads
    /// observe work published before that request. A concurrent request after
    /// this atomic operation remains set for the current or next drain.
    #[inline]
    #[must_use]
    pub fn take_host_wake(&self) -> bool {
        self.bits.fetch_and(!HOST_WAKE, Ordering::Acquire) & HOST_WAKE != 0
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
/// thread-safe channel before [`Self::request`] is called. The release/acquire
/// doorbell pair publishes preceding work, while the channel remains responsible
/// for race-free storage, ownership, and multi-producer ordering. Signaling the
/// bit does not wake a parked host execution thread. Unlike [`InterruptWord`],
/// this handle is [`Send`] and [`Sync`].
#[derive(Clone, Debug)]
pub struct HostWakeHandle {
    bits: Arc<AtomicU64>,
}

impl HostWakeHandle {
    /// Publishes preceding host work and requests a host-control exit.
    #[inline]
    pub fn request(&self) {
        self.bits.fetch_or(HOST_WAKE, Ordering::Release);
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

/// A sink wired directly to one validated guest line in one CPU interrupt word.
///
/// Holding this sink grants no access to either execution-control bit or to any
/// other guest line. One `(InterruptWord, line)` connection has exactly one
/// logical driver. This sink stores the aggregate level rather than a contribution
/// count, so constructing another sink for the same bit creates an invalid
/// topology: either sink could clear the other source's asserted level. Shared
/// sources use distinct interrupt-controller inputs, and only the controller owns
/// the direct output sink.
///
/// ```compile_fail
/// use se_core::interrupt::WordLineSink;
///
/// fn require_send<T: Send>() {}
///
/// require_send::<WordLineSink>();
/// ```
///
/// ```compile_fail
/// use se_core::interrupt::WordLineSink;
///
/// fn require_sync<T: Sync>() {}
///
/// require_sync::<WordLineSink>();
/// ```
#[derive(Debug)]
pub struct WordLineSink {
    word: InterruptWord,
    mask: u64,
}

impl WordLineSink {
    /// Connects a guest interrupt line to a CPU interrupt word.
    ///
    /// This constructor validates the bit assignment but cannot detect another
    /// sink created for the same word and line. Machine composition guarantees
    /// the single-logical-driver invariant.
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
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;

    use super::{
        EVENT_TRUNCATE, GUEST_INTERRUPT_MASK, HOST_WAKE, HostWakeHandle, InterruptLineError,
        InterruptSink, InterruptWord, WordLineSink,
    };

    #[test]
    fn host_wake_handle_remains_send_and_sync() {
        fn require_send_and_sync<T: Send + Sync>() {}

        require_send_and_sync::<HostWakeHandle>();
    }

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
        let sink = WordLineSink::new(word.clone(), 17).unwrap();
        sink.set(true);
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

    #[test]
    fn host_wake_acquire_observes_preceding_worker_publication() {
        const PAYLOAD: u64 = 0x1234_5678_9abc_def0;

        let word = InterruptWord::new();
        let wake = word.host_wake_handle();
        let payload = Arc::new(AtomicU64::new(0));
        let worker_payload = Arc::clone(&payload);
        let worker = thread::spawn(move || {
            worker_payload.store(PAYLOAD, Ordering::Relaxed);
            wake.request();
        });

        while word.load_relaxed() & HOST_WAKE == 0 {
            std::hint::spin_loop();
        }
        assert!(word.take_host_wake());
        assert_eq!(payload.load(Ordering::Relaxed), PAYLOAD);
        worker.join().unwrap();
    }
}
