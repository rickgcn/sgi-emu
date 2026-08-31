//! SGI Indigo IP12 hardware composition.

mod bus;
pub mod debug;
mod prom;

use std::error::Error;
use std::fmt;

use se_cpu::mips1::r3000::{R3000, R3000Config, StepError};
use se_device::rom::Rom;
use se_float::backend::Backend;

use self::bus::Ip12Bus;
use self::prom::normalize_u56_prom;

const PROM_BYTES: usize = 0x40000;

/// An error encountered while constructing an Indigo IP12.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ip12Error {
    /// The raw U56 PROM dump has an unsupported size.
    InvalidPromSize {
        /// Required raw image size in bytes.
        expected: usize,
        /// Supplied raw image size in bytes.
        actual: usize,
    },
}

impl fmt::Display for Ip12Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPromSize { expected, actual } => write!(
                formatter,
                "invalid IP12 PROM size: expected {expected} bytes, got {actual}"
            ),
        }
    }
}

impl Error for Ip12Error {}

/// An SGI Indigo IP12 with an R3000A and R3010.
pub struct Ip12 {
    cpu: R3000,
    bus: Ip12Bus,
}

impl Ip12 {
    /// Constructs an IP12 from a raw U56 PROM dump.
    ///
    /// # Errors
    ///
    /// Returns [`Ip12Error`] when the PROM image does not satisfy the IP12
    /// image contract.
    pub fn new(raw_prom: Vec<u8>, floating_point_backend: Backend) -> Result<Self, Ip12Error> {
        let prom = Rom::new(normalize_u56_prom(raw_prom)?);
        Ok(Self {
            cpu: R3000::new(cpu_config(floating_point_backend)),
            bus: Ip12Bus::new(prom),
        })
    }

    /// Restores the machine reset state.
    pub fn reset(&mut self) {
        self.cpu.reset();
    }

    /// Executes one architectural processor instruction.
    ///
    /// # Errors
    ///
    /// Returns [`StepError`] when the processor cannot complete the step.
    pub fn execute_instruction(&mut self) -> Result<(), StepError> {
        self.cpu.step(&mut self.bus)
    }

    /// Returns the virtual address of the next instruction to execute.
    #[must_use]
    pub fn execution_address(&self) -> u32 {
        self.cpu.program_counter()
    }
}

const fn cpu_config(floating_point_backend: Backend) -> R3000Config {
    R3000Config::new(32 * 1024, 32 * 1024, 64, 4, true, floating_point_backend)
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;

    use se_float::backend::Backend;

    use super::{Ip12, Ip12Error, PROM_BYTES, cpu_config};

    #[test]
    fn constructor_reports_invalid_prom_size() {
        assert!(matches!(
            Ip12::new(vec![0; PROM_BYTES - 1], Backend::SoftFloat),
            Err(Ip12Error::InvalidPromSize {
                expected: PROM_BYTES,
                actual
            }) if actual == PROM_BYTES - 1
        ));
    }

    #[test]
    fn cpu_configuration_matches_the_ip12_board() {
        for backend in [Backend::SoftFloat, Backend::Native] {
            let config = cpu_config(backend);
            assert_eq!(config.instruction_cache_bytes(), 32 * 1024);
            assert_eq!(config.data_cache_bytes(), 32 * 1024);
            assert_eq!(config.instruction_refill_bytes(), 64);
            assert_eq!(config.data_refill_bytes(), 4);
            assert!(config.partial_store_enabled());
            assert_eq!(config.floating_point_backend(), backend);
        }
    }

    #[test]
    #[ignore = "requires an external 070-8088-002 IP12 PROM dump"]
    fn reset_vector_reaches_reset_entry() {
        let path = env::var_os("SE_INDIGO_IP12_PROM")
            .expect("SE_INDIGO_IP12_PROM must name the external PROM dump");
        let raw_prom = fs::read(path).expect("the external PROM dump should be readable");
        let mut machine =
            Ip12::new(raw_prom, Backend::SoftFloat).expect("the PROM dump should be valid");

        assert_eq!(machine.cpu.program_counter(), 0xbfc0_0000);
        machine
            .execute_instruction()
            .expect("the reset jump should execute");
        machine
            .execute_instruction()
            .expect("the reset delay slot should execute");
        assert_eq!(machine.cpu.program_counter(), 0xbfc0_0200);
    }
}
