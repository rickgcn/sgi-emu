//! Converts absolute processor-clock phase into the machine nanosecond timeline.
//!
//! [`ProcessorClock`] is immutable profile configuration. [`RetirementPhase`]
//! stores only the PClk edge assigned to the next functional architectural CPU
//! transition; each boundary is derived with exact integer arithmetic.

use std::error::Error;
use std::fmt;
use std::num::NonZeroU64;

use se_core::time::{NS_PER_SEC, VTime};

// A higher rate would map distinct PClk edges to the same nanosecond.
const MAX_FREQUENCY_HZ: u64 = NS_PER_SEC;

/// Reports invalid PClk configuration or unrepresentable absolute CPU phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TimingError {
    /// A processor clock must have a nonzero rate.
    ZeroFrequency,
    /// Integer-nanosecond time cannot distinguish every requested PClk edge.
    FrequencyExceedsResolution { frequency_hz: u64, maximum_hz: u64 },
    /// The exact rounded-up boundary lies outside [`VTime`].
    BoundaryOverflow {
        next_pclk_tick: u64,
        frequency_hz: u64,
    },
    /// Advancing the authoritative PClk edge index would wrap.
    TickOverflow { next_pclk_tick: u64 },
    /// The absolute CPU phase precedes the current machine time.
    PhaseBehindMachine {
        next_boundary: VTime,
        machine_now: VTime,
    },
    /// A caller supplied a deadline earlier than current machine time.
    DeadlineBeforeMachine { deadline: VTime, machine_now: VTime },
}

impl fmt::Display for TimingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroFrequency => formatter.write_str("processor clock frequency is zero"),
            Self::FrequencyExceedsResolution {
                frequency_hz,
                maximum_hz,
            } => write!(
                formatter,
                "processor clock frequency {frequency_hz} Hz exceeds the {maximum_hz} Hz nanosecond representability limit"
            ),
            Self::BoundaryOverflow {
                next_pclk_tick,
                frequency_hz,
            } => write!(
                formatter,
                "PClk tick {next_pclk_tick} at {frequency_hz} Hz lies outside virtual time"
            ),
            Self::TickOverflow { next_pclk_tick } => write!(
                formatter,
                "PClk phase cannot advance beyond tick {next_pclk_tick}"
            ),
            Self::PhaseBehindMachine {
                next_boundary,
                machine_now,
            } => write!(
                formatter,
                "next CPU boundary {next_boundary} precedes machine time {machine_now}"
            ),
            Self::DeadlineBeforeMachine {
                deadline,
                machine_now,
            } => write!(
                formatter,
                "CPU deadline {deadline} precedes machine time {machine_now}"
            ),
        }
    }
}

impl Error for TimingError {}

/// Stores a processor PClk frequency in hertz.
///
/// Accepted frequencies are `1..=1_000_000_000` so consecutive PClk edges remain
/// distinguishable on the integer-nanosecond machine timeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ProcessorClock {
    frequency_hz: NonZeroU64,
}

impl ProcessorClock {
    /// Creates a clock whose transition boundaries are strictly increasing.
    ///
    /// # Errors
    ///
    /// Returns [`TimingError::ZeroFrequency`] for zero hertz or
    /// [`TimingError::FrequencyExceedsResolution`] above one gigahertz.
    pub(crate) fn new(frequency_hz: u64) -> Result<Self, TimingError> {
        let frequency_hz = NonZeroU64::new(frequency_hz).ok_or(TimingError::ZeroFrequency)?;
        if frequency_hz.get() > MAX_FREQUENCY_HZ {
            return Err(TimingError::FrequencyExceedsResolution {
                frequency_hz: frequency_hz.get(),
                maximum_hz: MAX_FREQUENCY_HZ,
            });
        }
        Ok(Self { frequency_hz })
    }

    /// Returns the rounded-up absolute nanosecond boundary for `phase`.
    ///
    /// The calculation is `ceil(tick * 1_000_000_000 / frequency_hz)` using exact
    /// integer arithmetic.
    ///
    /// # Errors
    ///
    /// Returns [`TimingError::BoundaryOverflow`] when the exact rounded result does
    /// not fit in [`VTime`].
    pub(crate) fn boundary(self, phase: RetirementPhase) -> Result<VTime, TimingError> {
        let tick = u128::from(phase.next_pclk_tick);
        let frequency_hz = u128::from(self.frequency_hz.get());
        let numerator =
            tick.checked_mul(u128::from(NS_PER_SEC))
                .ok_or(TimingError::BoundaryOverflow {
                    next_pclk_tick: phase.next_pclk_tick,
                    frequency_hz: self.frequency_hz.get(),
                })?;
        let quotient = numerator / frequency_hz;
        let remainder = numerator % frequency_hz;
        let rounded = quotient.checked_add(u128::from(remainder != 0)).ok_or(
            TimingError::BoundaryOverflow {
                next_pclk_tick: phase.next_pclk_tick,
                frequency_hz: self.frequency_hz.get(),
            },
        )?;
        VTime::try_from(rounded).map_err(|_| TimingError::BoundaryOverflow {
            next_pclk_tick: phase.next_pclk_tick,
            frequency_hz: self.frequency_hz.get(),
        })
    }
}

/// Holds the absolute PClk edge assigned to the next architectural transition.
///
/// The edge index is phase authority, not a retired-instruction counter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RetirementPhase {
    next_pclk_tick: u64,
}

impl RetirementPhase {
    /// Returns a phase whose next transition occupies the first modeled PClk edge.
    pub(crate) const fn initial() -> Self {
        Self { next_pclk_tick: 1 }
    }

    #[cfg(test)]
    pub(crate) const fn synthetic_test_state(next_pclk_tick: u64) -> Self {
        Self { next_pclk_tick }
    }

    pub(crate) const fn next_pclk_tick(self) -> u64 {
        self.next_pclk_tick
    }

    /// Returns the successor phase without modifying this value.
    ///
    /// # Errors
    ///
    /// Returns [`TimingError::TickOverflow`] when the edge index cannot advance.
    pub(crate) fn advanced(self) -> Result<Self, TimingError> {
        let next_pclk_tick =
            self.next_pclk_tick
                .checked_add(1)
                .ok_or(TimingError::TickOverflow {
                    next_pclk_tick: self.next_pclk_tick,
                })?;
        Ok(Self { next_pclk_tick })
    }
}

#[cfg(test)]
mod tests {
    use super::{ProcessorClock, RetirementPhase, TimingError};

    #[test]
    fn clock_rejects_zero_and_sub_nanosecond_transition_spacing() {
        assert_eq!(ProcessorClock::new(0), Err(TimingError::ZeroFrequency));
        assert_eq!(
            ProcessorClock::new(1_000_000_001),
            Err(TimingError::FrequencyExceedsResolution {
                frequency_hz: 1_000_000_001,
                maximum_hz: 1_000_000_000,
            })
        );
        assert!(ProcessorClock::new(1_000_000_000).is_ok());
    }

    #[test]
    fn one_hundred_eighty_megahertz_uses_exact_ceil_boundaries() {
        let clock = ProcessorClock::new(180_000_000).unwrap();
        let actual: Vec<_> = (1..=9)
            .map(|tick| {
                clock
                    .boundary(RetirementPhase::synthetic_test_state(tick))
                    .unwrap()
            })
            .collect();

        assert_eq!(actual, [6, 12, 17, 23, 28, 34, 39, 45, 50]);
    }

    #[test]
    fn representative_supported_rates_have_strictly_increasing_boundaries() {
        for frequency_hz in [1, 180_000_000, 333_333_333, 999_999_999, 1_000_000_000] {
            let clock = ProcessorClock::new(frequency_hz).unwrap();
            let mut previous = 0;
            for tick in 1..=1_000 {
                let boundary = clock
                    .boundary(RetirementPhase::synthetic_test_state(tick))
                    .unwrap();
                assert!(boundary > previous);
                previous = boundary;
            }
        }
    }

    #[test]
    fn timing_overflow_is_profile_independent() {
        let slow_clock = ProcessorClock::new(1).unwrap();
        assert!(matches!(
            slow_clock.boundary(RetirementPhase::synthetic_test_state(u64::MAX)),
            Err(TimingError::BoundaryOverflow {
                next_pclk_tick: u64::MAX,
                frequency_hz: 1,
            })
        ));
        assert_eq!(
            RetirementPhase::synthetic_test_state(u64::MAX).advanced(),
            Err(TimingError::TickOverflow {
                next_pclk_tick: u64::MAX,
            })
        );
    }
}
