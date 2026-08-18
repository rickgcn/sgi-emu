//! Adapts external R10000 interrupt-line levels to the architectural IP view.
//!
//! The five supported external inputs occupy guest lines zero through four in
//! [`se_core::interrupt::InterruptWord`] and appear as `Cause.IP2` through
//! `Cause.IP6`. The adapter parses a caller-provided snapshot and stores no
//! pending-state copy. Guest line bits carry no published host payload, so the
//! snapshot does not require acquire ordering.

const EXTERNAL_LINE_MASK: u64 = 0x1f;
const EXTERNAL_IP_SHIFT: u32 = 2;

/// Returns the unmasked `Cause.IP2` through `Cause.IP6` view of external line levels.
pub(crate) const fn external_pending_ip(pending: u64) -> u8 {
    let external_lines = pending & EXTERNAL_LINE_MASK;
    (external_lines as u8) << EXTERNAL_IP_SHIFT
}

#[cfg(test)]
mod tests {
    use se_core::interrupt::{InterruptSink, InterruptWord, WordLineSink};

    use super::external_pending_ip;

    #[test]
    fn five_external_lines_map_to_ip2_through_ip6() {
        for line in 0..5 {
            let word = InterruptWord::new();
            let sink = WordLineSink::new(word.clone(), line).unwrap();
            sink.set(true);

            assert_eq!(external_pending_ip(word.load_relaxed()), 1_u8 << (line + 2));

            sink.set(false);
            assert_eq!(external_pending_ip(word.load_relaxed()), 0);
        }
    }

    #[test]
    fn higher_guest_lines_do_not_feed_ip7_or_external_ip() {
        for line in [5, 6, 17, 61] {
            let word = InterruptWord::new();
            let sink = WordLineSink::new(word.clone(), line).unwrap();
            sink.set(true);

            assert_eq!(external_pending_ip(word.load_relaxed()), 0);
        }
    }
}
