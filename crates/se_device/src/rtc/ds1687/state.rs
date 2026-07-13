//! Serializable DS1687 runtime and battery state.

use core::fmt;
use std::collections::VecDeque;

use se_core::scheduler::SimTime;

use super::{
    DS1687_IRQ_OUTPUT, Ds1687, Ds1687Action, IrqSource, IrqTransaction, REGISTER_A, REGISTER_C,
    REGISTER_D,
};

const NVRAM_SIZE: usize = 256;

/// Complete DS1687 state required for an exact machine restore.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Ds1687State {
    initial_unix_seconds: i64,
    base_time: SimTime,
    registers: Vec<u8>,
    last_observed_second: i64,
    irq_asserted: bool,
    pending_irq_levels: Vec<bool>,
    persistence_revision: u64,
}

/// Battery-backed state shared across normal machine sessions.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Ds1687PersistentState {
    unix_seconds: i64,
    nvram: Vec<u8>,
    revision: u64,
}

impl Ds1687PersistentState {
    /// Creates validated persistent state.
    pub fn new(unix_seconds: i64, nvram: Vec<u8>, revision: u64) -> Result<Self, Ds1687StateError> {
        validate_nvram(&nvram)?;
        Ok(Self {
            unix_seconds,
            nvram,
            revision,
        })
    }

    /// Returns the saved logical UTC time.
    pub const fn unix_seconds(&self) -> i64 {
        self.unix_seconds
    }

    /// Returns the complete battery-backed register image.
    pub fn nvram(&self) -> &[u8] {
        &self.nvram
    }

    /// Returns the saved persistence revision.
    pub const fn revision(&self) -> u64 {
        self.revision
    }
}

/// Invalid serialized DS1687 state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Ds1687StateError {
    /// The register image does not contain exactly 256 bytes.
    InvalidNvramSize { size: usize },
}

impl fmt::Display for Ds1687StateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidNvramSize { size } => {
                write!(formatter, "invalid DS1687 NVRAM state size {size}")
            }
        }
    }
}

impl std::error::Error for Ds1687StateError {}

impl Ds1687 {
    /// Captures exact RTC state including pending IRQ deliveries.
    pub fn save_state(&self) -> Ds1687State {
        Ds1687State {
            initial_unix_seconds: self.initial_unix_seconds,
            base_time: self.base_time,
            registers: self.registers.to_vec(),
            last_observed_second: self.last_observed_second,
            irq_asserted: self.irq_asserted,
            pending_irq_levels: self
                .actions
                .iter()
                .map(|action| match action {
                    Ds1687Action::SetIrq(transaction) => transaction.asserted,
                    Ds1687Action::Idle => unreachable!("idle actions are never queued"),
                })
                .collect(),
            persistence_revision: self.persistence_revision,
        }
    }

    /// Restores exact RTC state after validating the register image.
    pub fn restore_state(&mut self, state: Ds1687State) -> Result<(), Ds1687StateError> {
        let registers = register_array(state.registers)?;
        self.initial_unix_seconds = state.initial_unix_seconds;
        self.base_time = state.base_time;
        self.registers = registers;
        self.last_observed_second = state.last_observed_second;
        self.irq_asserted = state.irq_asserted;
        self.actions = state
            .pending_irq_levels
            .into_iter()
            .map(|asserted| {
                Ds1687Action::SetIrq(IrqTransaction {
                    source: IrqSource {
                        component: self.id,
                        output: DS1687_IRQ_OUTPUT,
                    },
                    asserted,
                })
            })
            .collect::<VecDeque<_>>();
        self.persistence_revision = state.persistence_revision;
        Ok(())
    }

    /// Captures the battery-backed image at one simulated instant.
    pub fn persistent_state(&self, now: SimTime) -> Ds1687PersistentState {
        Ds1687PersistentState {
            unix_seconds: self.unix_seconds(now),
            nvram: self.registers.to_vec(),
            revision: self.persistence_revision,
        }
    }

    /// Applies battery-backed state to a newly constructed RTC.
    pub fn restore_persistent_state(
        &mut self,
        state: &Ds1687PersistentState,
        now: SimTime,
    ) -> Result<(), Ds1687StateError> {
        let mut registers = register_array(state.nvram.clone())?;
        registers[REGISTER_A] &= 0x7f;
        registers[REGISTER_C] = 0;
        registers[REGISTER_D] |= 0x80;
        self.initial_unix_seconds = state.unix_seconds;
        self.base_time = now;
        self.registers = registers;
        self.last_observed_second = state.unix_seconds;
        self.irq_asserted = false;
        self.actions.clear();
        self.persistence_revision = state.revision;
        Ok(())
    }
}

fn validate_nvram(nvram: &[u8]) -> Result<(), Ds1687StateError> {
    if nvram.len() != NVRAM_SIZE {
        return Err(Ds1687StateError::InvalidNvramSize { size: nvram.len() });
    }
    Ok(())
}

fn register_array(nvram: Vec<u8>) -> Result<[u8; NVRAM_SIZE], Ds1687StateError> {
    nvram
        .try_into()
        .map_err(|value: Vec<u8>| Ds1687StateError::InvalidNvramSize { size: value.len() })
}

#[cfg(test)]
mod tests {
    use se_core::component::ComponentId;

    use super::*;
    use crate::rtc::ds1687::Ds1687Config;

    #[test]
    fn exact_and_persistent_states_round_trip_independently() {
        let mut rtc = Ds1687::new(
            ComponentId::new(1),
            "RTC",
            1_000_000_000,
            Ds1687Config::default(),
        )
        .unwrap();
        rtc.registers[0x20] = 0x5a;
        rtc.persistence_revision = 7;
        let exact = rtc.save_state();
        let persistent = rtc.persistent_state(SimTime::new(2_000_000_000));

        rtc.registers[0x20] = 0;
        rtc.restore_state(exact).unwrap();
        assert_eq!(rtc.registers[0x20], 0x5a);

        let mut fresh = Ds1687::new(
            ComponentId::new(1),
            "RTC",
            1_000_000_000,
            Ds1687Config::default(),
        )
        .unwrap();
        fresh
            .restore_persistent_state(&persistent, SimTime::new(10))
            .unwrap();
        assert_eq!(fresh.registers[0x20], 0x5a);
        assert_eq!(fresh.persistence_revision(), 7);
    }
}
