//! Virtual-time definitions shared by every machine component.

/// Virtual time in nanoseconds.
pub type VTime = u64;

/// Nanoseconds in one microsecond.
pub const NS_PER_US: VTime = 1_000;

/// Nanoseconds in one millisecond.
pub const NS_PER_MS: VTime = 1_000_000;

/// Nanoseconds in one second.
pub const NS_PER_SEC: VTime = 1_000_000_000;

/// Sentinel used when a CPU burst has no event deadline.
pub const NO_DEADLINE: VTime = VTime::MAX;

#[cfg(test)]
mod tests {
    use super::{NO_DEADLINE, NS_PER_MS, NS_PER_SEC, NS_PER_US};

    #[test]
    fn time_units_are_exact_nanosecond_multiples() {
        assert_eq!(NS_PER_MS, NS_PER_US * 1_000);
        assert_eq!(NS_PER_SEC, NS_PER_MS * 1_000);
        assert_eq!(NO_DEADLINE, u64::MAX);
    }
}
