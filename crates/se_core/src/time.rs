//! Frontend-neutral virtual time values.

/// The number of attoseconds in one second.
pub const ATTOSECONDS_PER_SECOND: u128 = 1_000_000_000_000_000_000;

/// A duration measured in guest virtual time.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VirtualDuration {
    attoseconds: u128,
}

impl VirtualDuration {
    /// A duration with no elapsed time.
    pub const ZERO: Self = Self { attoseconds: 0 };

    /// Creates a duration from an attosecond count.
    #[must_use]
    pub const fn from_attoseconds(attoseconds: u128) -> Self {
        Self { attoseconds }
    }

    /// Returns the duration as an attosecond count.
    #[must_use]
    pub const fn as_attoseconds(self) -> u128 {
        self.attoseconds
    }

    /// Reports whether the duration contains no elapsed time.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.attoseconds == 0
    }
}

/// A point on the guest virtual timeline.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VirtualInstant {
    attoseconds: u128,
}

impl VirtualInstant {
    /// The origin of the guest virtual timeline.
    pub const ZERO: Self = Self { attoseconds: 0 };

    /// Returns the instant as an attosecond count from the timeline origin.
    #[must_use]
    pub const fn as_attoseconds(self) -> u128 {
        self.attoseconds
    }

    /// Advances the instant by one virtual duration.
    pub fn advance(&mut self, elapsed: VirtualDuration) {
        self.attoseconds += elapsed.as_attoseconds();
    }
}

#[cfg(test)]
mod tests {
    use super::{VirtualDuration, VirtualInstant};

    #[test]
    fn duration_preserves_attoseconds() {
        let duration = VirtualDuration::from_attoseconds(123_456);

        assert_eq!(duration.as_attoseconds(), 123_456);
        assert!(!duration.is_zero());
        assert!(VirtualDuration::ZERO.is_zero());
    }

    #[test]
    fn instant_accumulates_durations_exactly() {
        let mut instant = VirtualInstant::ZERO;

        instant.advance(VirtualDuration::from_attoseconds(17));
        instant.advance(VirtualDuration::from_attoseconds(29));

        assert_eq!(instant.as_attoseconds(), 46);
    }
}
