//! Deterministic conversion from CRIME cycles to machine time.

use se_core::component::{ComponentId, ComponentStateError};
use se_core::scheduler::{FractionalClockProjection, SimDuration};

const CRIME_FREQUENCY_HZ: u64 = 66_666_500;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CrimeClock {
    timebase_hz: u64,
    remainder: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(super) struct CrimeClockState {
    timebase_hz: u64,
    remainder: u64,
}

impl CrimeClock {
    pub(super) const fn new(timebase_hz: u64) -> Self {
        assert!(timebase_hz != 0, "the machine timebase must be nonzero");
        Self {
            timebase_hz,
            remainder: 0,
        }
    }

    pub(super) fn reset(&mut self) {
        self.remainder = 0;
    }

    pub(super) const fn save_state(self) -> CrimeClockState {
        CrimeClockState {
            timebase_hz: self.timebase_hz,
            remainder: self.remainder,
        }
    }

    pub(super) fn validate_state(
        self,
        component: ComponentId,
        state: CrimeClockState,
    ) -> Result<(), ComponentStateError> {
        if state.timebase_hz != self.timebase_hz {
            return Err(ComponentStateError::ConfigurationMismatch {
                component,
                field: "CRIME clock timebase",
            });
        }
        if state.remainder >= CRIME_FREQUENCY_HZ {
            return Err(ComponentStateError::InvalidState {
                component,
                invariant: "CRIME clock remainder must be normalized",
            });
        }
        Ok(())
    }

    pub(super) fn restore_state(&mut self, state: CrimeClockState) {
        self.remainder = state.remainder;
    }

    pub(super) fn next_cycle(&mut self) -> SimDuration {
        let base = self.timebase_hz / CRIME_FREQUENCY_HZ;
        self.remainder += self.timebase_hz % CRIME_FREQUENCY_HZ;
        let carry = self.remainder / CRIME_FREQUENCY_HZ;
        self.remainder %= CRIME_FREQUENCY_HZ;
        SimDuration::new(base + carry)
    }

    pub(super) fn projection(self) -> FractionalClockProjection {
        FractionalClockProjection::new(self.timebase_hz, CRIME_FREQUENCY_HZ, self.remainder)
    }

    pub(super) fn advance_cycles(&mut self, cycles: u64) -> SimDuration {
        let mut projection = self.projection();
        let elapsed = projection
            .advance(cycles)
            .expect("a CRIME clock advance must fit simulated time");
        self.remainder = projection.remainder();
        elapsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_conversion_retains_fractional_remainder() {
        let mut clock = CrimeClock::new(1_000_000_000);
        let elapsed = (0..66_666_500)
            .map(|_| clock.next_cycle().get())
            .sum::<u64>();
        assert_eq!(elapsed, 1_000_000_000);
    }
}
