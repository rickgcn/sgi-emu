//! SGI Indigo IP12 hardware composition.

mod bus;
pub mod debug;
mod prom;

use std::error::Error;
use std::fmt;

use se_cpu::mips1::r3000::{R3000, R3000Config, StepError};
use se_device::hpc1::Hpc1;
use se_device::int2::Int2;
use se_device::pic1::Pic1;
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
            bus: Ip12Bus::new(Pic1::new(0xf7, 2, true), Hpc1::new(), Int2::new(), prom),
        })
    }

    /// Restores the machine reset state.
    pub fn reset(&mut self) {
        self.cpu.reset();
        self.bus.reset();
    }

    /// Executes one architectural processor instruction.
    ///
    /// # Errors
    ///
    /// Returns [`StepError`] when the processor cannot complete the step.
    pub fn execute_instruction(&mut self) -> Result<(), StepError> {
        self.cpu.step(&mut self.bus)?;
        if self.bus.take_system_reset_request() {
            self.reset();
        }
        Ok(())
    }

    /// Returns the virtual address of the next instruction to execute.
    #[must_use]
    pub fn execution_address(&self) -> u32 {
        self.cpu.program_counter()
    }
}

const fn cpu_config(floating_point_backend: Backend) -> R3000Config {
    R3000Config::new(32 * 1024, 32 * 1024, 64, 16, true, floating_point_backend)
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;

    use se_core::bus::{PhysAddr, PhysicalBus};
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
            let mut machine = Ip12::new(vec![0; PROM_BYTES], backend).unwrap();
            let reset_configuration = read_word(&mut machine, 0x1fa0_0004) as u8;

            assert_eq!(reset_configuration & 0xf0, 0xf0);
            assert_eq!((reset_configuration >> 2) & 0x03, 0x01);
            assert_eq!(reset_configuration & 0x03, 0x03);
            assert_eq!(config.instruction_cache_bytes(), 32 * 1024);
            assert_eq!(config.data_cache_bytes(), 32 * 1024);
            assert_eq!(config.instruction_refill_bytes(), 16 * 4);
            assert_eq!(config.data_refill_bytes(), 4 * 4);
            assert!(config.partial_store_enabled());
            assert_eq!(config.floating_point_backend(), backend);
        }
    }

    #[test]
    fn reset_restores_the_cpu_and_asic_front_end_without_changing_prom() {
        let mut raw_prom = vec![0; PROM_BYTES];
        raw_prom[0x100..0x104].copy_from_slice(&[0x34, 0x12, 0x78, 0x56]);
        let mut machine = Ip12::new(raw_prom, Backend::SoftFloat).unwrap();

        machine.execute_instruction().unwrap();
        assert_eq!(machine.execution_address(), 0xbfc0_0004);
        machine
            .bus
            .write(PhysAddr::new(0x1faa_0000), &0x0123_4567_u32.to_be_bytes())
            .unwrap();
        machine
            .bus
            .write(PhysAddr::new(0x1fb8_00c3), &[0x1f])
            .unwrap();
        machine
            .bus
            .write(PhysAddr::new(0x1fb8_01c7), &[0xa5])
            .unwrap();

        machine.reset();

        assert_eq!(machine.execution_address(), 0xbfc0_0000);
        assert_eq!(read_word(&mut machine, 0x1faa_0000), 0);
        assert_eq!(read_word(&mut machine, 0x1fb8_00c0), 0x40);
        assert_eq!(read_word(&mut machine, 0x1fb8_01c4), 0);
        assert_eq!(read_word(&mut machine, 0x1fa0_0004), 0xf7);
        assert_eq!(read_word(&mut machine, 0x1fa0_0008), 0x88);
        assert_eq!(read_word(&mut machine, 0x1fc0_0100), 0x1234_5678);
    }

    #[test]
    fn guest_system_initialize_uses_the_machine_reset_path() {
        let mut machine = Ip12::new(vec![0; PROM_BYTES], Backend::SoftFloat).unwrap();
        machine
            .bus
            .write(PhysAddr::new(0x1fb8_00c3), &[0x1f])
            .unwrap();
        machine
            .bus
            .write(PhysAddr::new(0x1fa0_0000), &0x0000_0200_u32.to_be_bytes())
            .unwrap();

        machine.execute_instruction().unwrap();

        assert_eq!(machine.execution_address(), 0xbfc0_0000);
        assert_eq!(read_word(&mut machine, 0x1fb8_00c0), 0x40);
        assert_eq!(read_word(&mut machine, 0x1fa0_0000), 0);
        assert!(!machine.bus.take_system_reset_request());
    }

    fn read_word(machine: &mut Ip12, address: u64) -> u32 {
        let mut bytes = [0; 4];
        machine
            .bus
            .read(PhysAddr::new(address), &mut bytes)
            .unwrap();
        u32::from_be_bytes(bytes)
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

    #[test]
    #[ignore = "requires an external 070-8088-002 IP12 PROM dump"]
    fn reset_asic_front_end_reaches_first_subroutine_call() {
        let path = env::var_os("SE_INDIGO_IP12_PROM")
            .expect("SE_INDIGO_IP12_PROM must name the external PROM dump");
        let raw_prom = fs::read(path).expect("the external PROM dump should be readable");
        let mut machine =
            Ip12::new(raw_prom, Backend::SoftFloat).expect("the PROM dump should be valid");

        for _ in 0..256 {
            if machine.execution_address() == 0xbfc0_02f0 {
                return;
            }
            machine
                .execute_instruction()
                .expect("the reset ASIC front end should execute");
        }

        assert_eq!(machine.execution_address(), 0xbfc0_02f0);
    }
}
