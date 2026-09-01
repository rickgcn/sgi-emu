//! SGI Indigo IP12 hardware composition.

mod bus;
pub mod debug;
mod prom;

use std::error::Error;
use std::fmt;

use se_core::time::VirtualDuration;
use se_cpu::mips1::r3000::{R3000, R3000Config, StepError};
use se_device::dp8573a::{Dp8573a, Dp8573aBatteryState, Dp8573aStateError};
use se_device::dsp56001::Dsp56001;
use se_device::hpc1::Hpc1;
use se_device::int2::Int2;
use se_device::mdac::Mdac;
use se_device::nmc93cs46::{Nmc93cs46, Nmc93cs46Contents};
use se_device::pic1::Pic1;
use se_device::ram::Ram;
use se_device::rom::Rom;
use se_device::wd33c93b::Wd33c93b;
use se_device::z85230::Z85230;
use se_float::backend::Backend;

use crate::output::MachineOutput;
use crate::serial::SerialPort;

use self::bus::Ip12Bus;
use self::prom::normalize_u56_prom;

const PROM_BYTES: usize = 0x40000;
const RAM_BYTES: usize = 8 * 1024 * 1024;
const CPU_FREQUENCY_HZ: u64 = 33_000_000;
const SERIAL_CLOCK_HZ: u64 = 3_686_400;

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

/// Nonvolatile state retained by an Indigo IP12.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ip12NonvolatileState {
    nvram: Nmc93cs46Contents,
    rtc: Dp8573aBatteryState,
}

/// Fixed-width data used to import or export Indigo IP12 nonvolatile state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ip12NonvolatileStateParts {
    /// Serial EEPROM words in address order.
    pub nvram_words: [u16; 64],
    /// Main DP8573A register storage.
    pub rtc_registers: [u8; 32],
    /// Alternate DP8573A control register storage.
    pub rtc_alternate_control_registers: [u8; 4],
    /// Sub-millisecond RTC prescaler phase in attoseconds.
    pub rtc_prescaler_phase_attoseconds: u64,
    /// RTC millisecond position within the current hundredth.
    pub rtc_millisecond_within_hundredth: u8,
    /// Whether the RTC oscillator-failed flag is set.
    pub rtc_oscillator_failed: bool,
    /// Whether the RTC uses single-supply operation.
    pub rtc_single_supply: bool,
    /// Whether the RTC alarm comparison is currently active.
    pub rtc_alarm_match_active: bool,
}

/// An invalid Indigo IP12 nonvolatile state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ip12NonvolatileStateError(Dp8573aStateError);

impl fmt::Display for Ip12NonvolatileStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid Indigo IP12 nonvolatile state: {}",
            self.0
        )
    }
}

impl Error for Ip12NonvolatileStateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.0)
    }
}

impl Ip12NonvolatileState {
    const fn new(nvram: Nmc93cs46Contents, rtc: Dp8573aBatteryState) -> Self {
        Self { nvram, rtc }
    }

    /// Returns a fixed-width representation without exposing device types.
    #[must_use]
    pub fn parts(&self) -> Ip12NonvolatileStateParts {
        Ip12NonvolatileStateParts {
            nvram_words: *self.nvram.words(),
            rtc_registers: *self.rtc.registers(),
            rtc_alternate_control_registers: *self.rtc.alternate_control_registers(),
            rtc_prescaler_phase_attoseconds: self.rtc.prescaler_phase_attoseconds(),
            rtc_millisecond_within_hundredth: self.rtc.millisecond_within_hundredth(),
            rtc_oscillator_failed: self.rtc.oscillator_failed(),
            rtc_single_supply: self.rtc.single_supply(),
            rtc_alarm_match_active: self.rtc.alarm_match_active(),
        }
    }

    /// Creates validated state from its fixed-width representation.
    ///
    /// # Errors
    ///
    /// Returns [`Ip12NonvolatileStateError`] when an RTC phase is outside its
    /// valid range.
    pub fn try_from_parts(
        parts: Ip12NonvolatileStateParts,
    ) -> Result<Self, Ip12NonvolatileStateError> {
        let rtc = Dp8573aBatteryState::new(
            parts.rtc_registers,
            parts.rtc_alternate_control_registers,
            parts.rtc_prescaler_phase_attoseconds,
            parts.rtc_millisecond_within_hundredth,
            parts.rtc_oscillator_failed,
            parts.rtc_single_supply,
            parts.rtc_alarm_match_active,
        )
        .map_err(Ip12NonvolatileStateError)?;
        Ok(Self::new(Nmc93cs46Contents::new(parts.nvram_words), rtc))
    }
}

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
            bus: Ip12Bus::new(
                Pic1::new(0xf7, 2, true),
                [Some(Ram::new(RAM_BYTES)), None, None, None],
                Hpc1::new(),
                Int2::new(),
                Wd33c93b::new(),
                [Z85230::new(SERIAL_CLOCK_HZ), Z85230::new(SERIAL_CLOCK_HZ)],
                Dp8573a::new(),
                Mdac::new(),
                Nmc93cs46::new(),
                Dsp56001::new(),
                prom,
            ),
        })
    }

    /// Returns the state retained across machine reconstruction and
    /// application sessions.
    #[must_use]
    pub fn nonvolatile_state(&self) -> Ip12NonvolatileState {
        let (nvram, rtc) = self.bus.nonvolatile_state();
        Ip12NonvolatileState::new(nvram, rtc)
    }

    /// Restores retained state and advances a running RTC by elapsed offline
    /// milliseconds.
    pub fn restore_nonvolatile_state(
        &mut self,
        state: Ip12NonvolatileState,
        offline_milliseconds: u64,
    ) {
        self.bus
            .restore_nonvolatile_state(state.nvram, state.rtc, offline_milliseconds);
        self.update_interrupt_lines();
    }

    /// Restores the machine reset state.
    pub fn reset(&mut self) {
        self.cpu.reset();
        self.bus.reset();
        self.update_interrupt_lines();
    }

    /// Returns the processor clock frequency in hertz.
    #[must_use]
    pub const fn cpu_frequency_hz(&self) -> u64 {
        self.cpu.frequency_hz()
    }

    /// Executes one architectural processor instruction.
    ///
    /// # Errors
    ///
    /// Returns [`StepError`] when the processor cannot complete the step.
    pub fn execute_instruction(&mut self) -> Result<(), StepError> {
        self.update_interrupt_lines();
        self.cpu.step(&mut self.bus)?;
        if self.bus.take_system_reset_request() {
            self.reset();
        }
        Ok(())
    }

    /// Advances timed devices and appends frontend-visible output.
    pub fn advance_time(&mut self, elapsed: VirtualDuration, output: &mut MachineOutput) {
        self.bus.advance_time(elapsed, output);
    }

    /// Supplies host bytes to one external serial receiver.
    ///
    /// Returns the number of bytes consumed by the machine.
    pub fn receive_serial(&mut self, port: SerialPort, bytes: &[u8]) -> usize {
        let consumed = self.bus.receive_serial(port, bytes);
        self.update_interrupt_lines();
        consumed
    }

    fn update_interrupt_lines(&mut self) {
        let mut interrupt_lines = u8::from(self.cpu.cp1_interrupt_asserted());
        if self.bus.local_interrupt_0_asserted() {
            interrupt_lines |= 1 << 1;
        }
        if self.bus.error_interrupt_asserted() {
            interrupt_lines |= 1 << 5;
        }
        self.cpu.set_hardware_interrupt_lines(interrupt_lines);
    }

    /// Returns the virtual address of the next instruction to execute.
    #[must_use]
    pub fn execution_address(&self) -> u32 {
        self.cpu.program_counter()
    }
}

const fn cpu_config(floating_point_backend: Backend) -> R3000Config {
    R3000Config::new(
        CPU_FREQUENCY_HZ,
        32 * 1024,
        32 * 1024,
        64,
        16,
        true,
        floating_point_backend,
    )
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;

    use crate::serial::SerialPort;
    use se_core::bus::{PhysAddr, PhysicalBus};
    use se_float::backend::Backend;

    use super::{
        CPU_FREQUENCY_HZ, Ip12, Ip12Error, Ip12NonvolatileState, Ip12NonvolatileStateParts,
        PROM_BYTES, RAM_BYTES, cpu_config,
    };

    const MEMORY_CONFIGURATION_INSTRUCTION_BUDGET: usize = 300_000;
    const STACK_SETUP_INSTRUCTION_BUDGET: usize = 30_000;

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
    fn nonvolatile_state_parts_round_trip_without_device_types() {
        let mut nvram_words = [u16::MAX; 64];
        nvram_words[7] = 0x1234;
        let mut rtc_registers = [0; 32];
        rtc_registers[6] = 0x42;
        let parts = Ip12NonvolatileStateParts {
            nvram_words,
            rtc_registers,
            rtc_alternate_control_registers: [0x08, 0x11, 0x22, 0x33],
            rtc_prescaler_phase_attoseconds: 500,
            rtc_millisecond_within_hundredth: 7,
            rtc_oscillator_failed: false,
            rtc_single_supply: true,
            rtc_alarm_match_active: false,
        };

        let state = Ip12NonvolatileState::try_from_parts(parts).unwrap();

        assert_eq!(state.parts(), parts);
    }

    #[test]
    fn nonvolatile_state_parts_validate_rtc_phases() {
        let base = Ip12NonvolatileStateParts {
            nvram_words: [u16::MAX; 64],
            rtc_registers: [0; 32],
            rtc_alternate_control_registers: [0; 4],
            rtc_prescaler_phase_attoseconds: 0,
            rtc_millisecond_within_hundredth: 0,
            rtc_oscillator_failed: false,
            rtc_single_supply: false,
            rtc_alarm_match_active: false,
        };

        assert!(
            Ip12NonvolatileState::try_from_parts(Ip12NonvolatileStateParts {
                rtc_prescaler_phase_attoseconds: 1_000_000_000_000_000,
                ..base
            })
            .is_err()
        );
        assert!(
            Ip12NonvolatileState::try_from_parts(Ip12NonvolatileStateParts {
                rtc_millisecond_within_hundredth: 10,
                ..base
            })
            .is_err()
        );
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
            assert_eq!(config.frequency_hz(), CPU_FREQUENCY_HZ);
            assert_eq!(machine.cpu_frequency_hz(), CPU_FREQUENCY_HZ);
            machine.reset();
            assert_eq!(machine.cpu_frequency_hz(), CPU_FREQUENCY_HZ);
        }
    }

    #[test]
    fn production_topology_contains_one_eight_megabyte_ram_module() {
        let mut machine = Ip12::new(vec![0; PROM_BYTES], Backend::SoftFloat).unwrap();
        machine
            .bus
            .write(PhysAddr::new(0x1fa1_0000), &0x0f00_023f_u32.to_be_bytes())
            .unwrap();

        machine
            .bus
            .write(
                PhysAddr::new(RAM_BYTES as u64 - 4),
                &0x0123_4567_u32.to_be_bytes(),
            )
            .unwrap();
        assert_eq!(read_word(&mut machine, RAM_BYTES as u64 - 4), 0x0123_4567);
        assert_eq!(read_word(&mut machine, RAM_BYTES as u64), 0);
    }

    #[test]
    fn reset_restores_the_cpu_and_asic_front_end_without_changing_ram_or_prom() {
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
        machine
            .bus
            .write(PhysAddr::new(0x1fb8_01bf), &[0x0f])
            .unwrap();
        machine
            .bus
            .write(PhysAddr::new(0x1fb8_0e57), &[0xa5])
            .unwrap();
        machine
            .bus
            .write(PhysAddr::new(0x1fa1_0000), &0x0100_023f_u32.to_be_bytes())
            .unwrap();
        machine
            .bus
            .write(PhysAddr::new(0x0060_0000), &0x89ab_cdef_u32.to_be_bytes())
            .unwrap();
        machine.bus.write(PhysAddr::new(0x00c0_0000), &[0]).unwrap();
        assert!(machine.bus.error_interrupt_asserted());
        machine.execute_instruction().unwrap();
        assert_ne!(
            machine.cpu.debug_snapshot().cp0.registers[13] & (1 << 15),
            0
        );

        machine.reset();

        assert_eq!(machine.execution_address(), 0xbfc0_0000);
        assert_eq!(read_word(&mut machine, 0x1faa_0000), 0);
        assert_eq!(read_word(&mut machine, 0x1fb8_00c0), 0x40);
        assert_eq!(read_word(&mut machine, 0x1fb8_01c4), 0);
        assert_eq!(read_byte(&mut machine, 0x1fb8_01bf), 0);
        assert_eq!(read_byte(&mut machine, 0x1fb8_0e57), 0xa5);
        assert_eq!(read_word(&mut machine, 0x1fa0_0004), 0xf7);
        assert_eq!(read_word(&mut machine, 0x1fa0_0008), 0x88);
        assert_eq!(read_word(&mut machine, 0x1fa1_0000), 0);
        assert!(!machine.bus.error_interrupt_asserted());
        assert_eq!(
            machine.cpu.debug_snapshot().cp0.registers[13] & (1 << 15),
            0
        );
        machine
            .bus
            .write(PhysAddr::new(0x1fa1_0000), &0x0100_023f_u32.to_be_bytes())
            .unwrap();
        assert_eq!(read_word(&mut machine, 0x0060_0000), 0x89ab_cdef);
        assert_eq!(read_word(&mut machine, 0x1fc0_0100), 0x1234_5678);
    }

    #[test]
    fn pic1_error_output_drives_and_releases_cpu_interrupt_input_five() {
        let mut machine = Ip12::new(vec![0; PROM_BYTES], Backend::SoftFloat).unwrap();
        machine
            .bus
            .write(
                PhysAddr::new(4 * 1024 * 1024),
                &0x0123_4567_u32.to_be_bytes(),
            )
            .unwrap();

        machine.execute_instruction().unwrap();
        assert_ne!(
            machine.cpu.debug_snapshot().cp0.registers[13] & (1 << 15),
            0
        );

        machine.bus.write(PhysAddr::new(0x1fa1_0210), &[0]).unwrap();
        machine.execute_instruction().unwrap();
        assert_eq!(
            machine.cpu.debug_snapshot().cp0.registers[13] & (1 << 15),
            0
        );
    }

    #[test]
    fn guest_store_error_is_sampled_at_the_next_instruction_boundary() {
        let mut machine = machine_with_instructions(&[0x3c08_a040, 0xad00_0000, 0]);

        machine.execute_instruction().unwrap();
        machine.execute_instruction().unwrap();
        assert!(machine.bus.error_interrupt_asserted());
        assert_eq!(
            machine.cpu.debug_snapshot().cp0.registers[13] & (1 << 15),
            0
        );

        machine.execute_instruction().unwrap();
        assert_ne!(
            machine.cpu.debug_snapshot().cp0.registers[13] & (1 << 15),
            0
        );
    }

    #[test]
    fn cp1_error_output_drives_cpu_interrupt_input_zero() {
        let mut machine = machine_with_instructions(&[
            0x3c08_2040,
            0x4088_6000,
            0,
            0x3c08_0002,
            0x44c8_f800,
            0,
            0,
        ]);

        for _ in 0..6 {
            machine.execute_instruction().unwrap();
        }
        assert!(machine.cpu.cp1_interrupt_asserted());
        assert_eq!(
            machine.cpu.debug_snapshot().cp0.registers[13] & (1 << 10),
            0
        );

        machine.execute_instruction().unwrap();
        assert_ne!(
            machine.cpu.debug_snapshot().cp0.registers[13] & (1 << 10),
            0
        );
    }

    #[test]
    fn serial_receive_interrupt_drives_cpu_interrupt_input_one() {
        let mut machine = Ip12::new(vec![0; PROM_BYTES], Backend::SoftFloat).unwrap();
        for (register, value) in [(3, 1), (1, 0x10), (9, 1 << 3)] {
            machine
                .bus
                .write(PhysAddr::new(0x1fb8_0d1b), &[register])
                .unwrap();
            machine
                .bus
                .write(PhysAddr::new(0x1fb8_0d1b), &[value])
                .unwrap();
        }
        machine
            .bus
            .write(PhysAddr::new(0x1fb8_01c7), &[1 << 5])
            .unwrap();

        assert_eq!(machine.receive_serial(SerialPort::A, b"A"), 1);
        assert_ne!(
            machine.cpu.debug_snapshot().cp0.registers[13] & (1 << 11),
            0
        );
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

    fn read_byte(machine: &mut Ip12, address: u64) -> u8 {
        let mut byte = [0];
        machine.bus.read(PhysAddr::new(address), &mut byte).unwrap();
        byte[0]
    }

    fn machine_with_instructions(instructions: &[u32]) -> Ip12 {
        let mut raw_prom = vec![0; PROM_BYTES];
        for (destination, instruction) in raw_prom
            .chunks_exact_mut(4)
            .zip(instructions.iter().copied())
        {
            let [first, second, third, fourth] = instruction.to_be_bytes();
            destination.copy_from_slice(&[second, first, fourth, third]);
        }
        Ip12::new(raw_prom, Backend::SoftFloat).unwrap()
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

    #[test]
    #[ignore = "requires an external 070-8088-002 IP12 PROM dump"]
    fn board_diagnostics_reach_memory_initialization() {
        let path = env::var_os("SE_INDIGO_IP12_PROM")
            .expect("SE_INDIGO_IP12_PROM must name the external PROM dump");
        let raw_prom = fs::read(path).expect("the external PROM dump should be readable");
        let mut machine =
            Ip12::new(raw_prom, Backend::SoftFloat).expect("the PROM dump should be valid");

        for _ in 0..20_000 {
            if machine.execution_address() == 0xbfc0_0320 {
                return;
            }
            machine
                .execute_instruction()
                .expect("the board diagnostics should execute");
        }

        assert_eq!(machine.execution_address(), 0xbfc0_0320);
    }

    #[test]
    #[ignore = "requires an external 070-8088-002 IP12 PROM dump"]
    fn memory_initialization_reaches_stack_setup() {
        let path = env::var_os("SE_INDIGO_IP12_PROM")
            .expect("SE_INDIGO_IP12_PROM must name the external PROM dump");
        let raw_prom = fs::read(path).expect("the external PROM dump should be readable");
        let mut machine =
            Ip12::new(raw_prom, Backend::SoftFloat).expect("the PROM dump should be valid");

        machine
            .bus
            .write(PhysAddr::new(0x1fa1_0000), &0x0100_003f_u32.to_be_bytes())
            .unwrap();
        for address in (0x0038_0000..0x0040_0000).step_by(4) {
            machine
                .bus
                .write(PhysAddr::new(address), &0xa5a5_a5a5_u32.to_be_bytes())
                .unwrap();
        }
        machine.reset();

        execute_until(
            &mut machine,
            0xbfc0_0328,
            MEMORY_CONFIGURATION_INSTRUCTION_BUDGET,
        );
        assert_eq!(read_word(&mut machine, 0x1fa1_0000), 0x0100_003f);
        assert_eq!(read_word(&mut machine, 0x1fa1_0004), 0x003f_003f);
        assert_ne!(read_word(&mut machine, 0x1fa0_0000) & 0x400, 0);
        assert!(!machine.bus.error_interrupt_asserted());

        for offset in (0..0x28).step_by(4) {
            assert_eq!(
                read_word(&mut machine, offset),
                read_word(&mut machine, 0x1fc0_0950 + offset)
            );
        }
        for address in (0x0038_0000..0x0040_0000).step_by(4) {
            assert_eq!(read_word(&mut machine, address), 0);
        }

        execute_until(&mut machine, 0xbfc0_0fb0, STACK_SETUP_INSTRUCTION_BUDGET);
        assert_eq!(read_word(&mut machine, 0x0038_21c0), 0xfeed_dead);
        assert_eq!(read_word(&mut machine, 0x0038_21c4), 0xa03f_fff0);
        let stack_pointer = machine.cpu.debug_snapshot().gpr[29];
        assert!((0xa038_0000..0xa040_0000).contains(&stack_pointer));
        assert!(!machine.bus.error_interrupt_asserted());
    }

    fn execute_until(machine: &mut Ip12, target: u32, budget: usize) {
        for _ in 0..budget {
            if machine.execution_address() == target {
                return;
            }
            machine
                .execute_instruction()
                .expect("the PROM memory initialization should execute");
        }

        panic!(
            "instruction budget exhausted at 0x{:08x}, expected 0x{target:08x}",
            machine.execution_address()
        )
    }
}
