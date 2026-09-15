//! SGI Indigo IP12 hardware composition.

pub mod builder;
mod bus;
pub mod debug;
pub mod definition;
mod events;
pub mod plan;
mod prom;
pub(crate) mod snapshot;

use std::error::Error;
use std::fmt;

use se_core::storage::StorageMedium;
use se_core::time::VirtualDuration;
use se_cpu::mips1::r3000::{R3000, R3000Config, StepError};
use se_device::centronics::CentronicsPort;
use se_device::dp8573a::{Dp8573a, Dp8573aBatteryState, Dp8573aStateError};
use se_device::dsp56001::Dsp56001;
use se_device::gio::{GioBus, GioSnapshotError};
use se_device::hpc1::Hpc1;
use se_device::int2::Int2;
use se_device::mdac::Mdac;
use se_device::nmc93cs46::{Nmc93cs46, Nmc93cs46Contents};
use se_device::pic1::Pic1;
use se_device::ram::Ram;
use se_device::rom::Rom;
use se_device::scsi::{ScsiAttachError, ScsiBus, ScsiSnapshotError};
use se_device::scsi_cdrom::ScsiCdrom;
use se_device::scsi_disk::ScsiDisk;
use se_device::seeq8003::Seeq8003;
use se_device::sgi_keyboard::{SgiKey, SgiKeyboard};
use se_device::sgi_mouse::{SgiMouse, SgiMouseButton};
use se_device::wd33c93b::Wd33c93b;
use se_device::z85230::Z85230;
use se_float::backend::Backend;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use self::bus::Ip12Bus;
use self::plan::Ip12Port;
use self::prom::{normalize_u56_prom, validate_u56_prom_size};
use crate::endpoint::{
    EndpointCatalog, EndpointDescriptor, EndpointDirection, EndpointKey, EndpointKind,
};
use crate::input::{
    KeyboardKey, KeyboardNamedKey, MachineInput, MachineInputPayload, PointerButton,
};
use crate::machine::{MachineInputError, MachineInputResult};
use crate::output::{MachineOutput, VideoOutput};
use se_device::z85230::Channel;

const PROM_BYTES: usize = 0x40000;
#[cfg(test)]
const RAM_BYTES: usize = 8 * 1024 * 1024;
const CPU_FREQUENCY_HZ: u64 = 33_000_000;
const SERIAL_CLOCK_HZ: u64 = 3_686_400;
// HP-1 schematic, sheet 15: U72 CLK is connected to CLK.20.
const SCSI_CLOCK_HZ: u64 = 20_000_000;

/// Capacity of one SIMM installed in an Indigo IP12 memory bank.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Ip12SimmSize {
    /// A 2 MiB SIMM.
    Mib2,
    /// A 4 MiB SIMM.
    Mib4,
    /// An 8 MiB SIMM.
    Mib8,
}

impl Ip12SimmSize {
    /// Returns the SIMM capacity in mebibytes.
    #[must_use]
    pub const fn simm_mib(self) -> u8 {
        match self {
            Self::Mib2 => 2,
            Self::Mib4 => 4,
            Self::Mib8 => 8,
        }
    }
}

/// Installed memory banks for an Indigo IP12.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ip12MemoryConfiguration {
    banks: [Option<Ip12SimmSize>; 3],
}

impl Ip12MemoryConfiguration {
    /// Creates a configuration from banks A, B, and C.
    ///
    /// # Errors
    ///
    /// Returns [`Ip12MemoryConfigurationError::NoInstalledBank`] when all
    /// three banks are empty.
    pub const fn new(
        banks: [Option<Ip12SimmSize>; 3],
    ) -> Result<Self, Ip12MemoryConfigurationError> {
        if banks[0].is_none() && banks[1].is_none() && banks[2].is_none() {
            return Err(Ip12MemoryConfigurationError::NoInstalledBank);
        }
        Ok(Self { banks })
    }

    /// Parses the per-SIMM capacities for banks A, B, and C.
    ///
    /// Zero denotes an empty bank. Installed banks accept 2, 4, or 8 MiB
    /// SIMMs.
    ///
    /// # Errors
    ///
    /// Returns [`Ip12MemoryConfigurationError`] when a capacity is unsupported
    /// or every bank is empty.
    pub fn try_from_simm_mib(simm_mib: [u8; 3]) -> Result<Self, Ip12MemoryConfigurationError> {
        let mut banks = [None; 3];
        for (index, capacity) in simm_mib.into_iter().enumerate() {
            banks[index] = match capacity {
                0 => None,
                2 => Some(Ip12SimmSize::Mib2),
                4 => Some(Ip12SimmSize::Mib4),
                8 => Some(Ip12SimmSize::Mib8),
                _ => {
                    return Err(Ip12MemoryConfigurationError::UnsupportedSimmCapacity {
                        bank: index,
                        mib: capacity,
                    });
                }
            };
        }
        Self::new(banks)
    }

    /// Returns banks A, B, and C in socket-group order.
    #[must_use]
    pub const fn banks(self) -> [Option<Ip12SimmSize>; 3] {
        self.banks
    }

    /// Returns each bank's installed per-SIMM capacity in mebibytes.
    ///
    /// Empty banks are represented by zero.
    #[must_use]
    pub const fn simm_mib(self) -> [u8; 3] {
        [
            simm_mib(self.banks[0]),
            simm_mib(self.banks[1]),
            simm_mib(self.banks[2]),
        ]
    }
}

impl Default for Ip12MemoryConfiguration {
    fn default() -> Self {
        Self {
            banks: [Some(Ip12SimmSize::Mib2), None, None],
        }
    }
}

impl Serialize for Ip12MemoryConfiguration {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.banks.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Ip12MemoryConfiguration {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let banks = <[Option<Ip12SimmSize>; 3]>::deserialize(deserializer)?;
        Self::new(banks).map_err(serde::de::Error::custom)
    }
}

/// An invalid Indigo IP12 memory-bank configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ip12MemoryConfigurationError {
    /// All three banks are empty.
    NoInstalledBank,
    /// A bank specifies a SIMM capacity other than 2, 4, or 8 MiB.
    UnsupportedSimmCapacity {
        /// Zero-based bank index in A, B, C order.
        bank: usize,
        /// Unsupported per-SIMM capacity in mebibytes.
        mib: u8,
    },
}

impl fmt::Display for Ip12MemoryConfigurationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoInstalledBank => {
                formatter.write_str("invalid IP12 memory configuration: no bank is installed")
            }
            Self::UnsupportedSimmCapacity { bank, mib } => write!(
                formatter,
                "invalid IP12 memory bank {} SIMM capacity: expected 0, 2, 4, or 8 MiB, got {mib} MiB",
                bank + 1
            ),
        }
    }
}

impl Error for Ip12MemoryConfigurationError {}

const fn simm_mib(simm_size: Option<Ip12SimmSize>) -> u8 {
    match simm_size {
        Some(simm_size) => simm_size.simm_mib(),
        None => 0,
    }
}

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
    /// The optional disk cannot be represented by the fixed SCSI target.
    InvalidDiskSize {
        /// Supplied storage size in bytes.
        bytes: u64,
    },
    /// The optional CD-ROM cannot be represented by the fixed SCSI target.
    InvalidCdromSize {
        /// Supplied storage size in bytes.
        bytes: u64,
    },
    /// The fixed IP12 SCSI topology could not be assembled.
    ScsiAttachment(ScsiAttachError),
}

impl fmt::Display for Ip12Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPromSize { expected, actual } => write!(
                formatter,
                "invalid IP12 PROM size: expected {expected} bytes, got {actual}"
            ),
            Self::InvalidDiskSize { bytes } => write!(
                formatter,
                "invalid IP12 disk size: expected a nonzero multiple of 512 bytes representable by READ CAPACITY(10), got {bytes}"
            ),
            Self::InvalidCdromSize { bytes } => write!(
                formatter,
                "invalid IP12 CD-ROM size: expected a nonzero multiple of 2048 bytes representable as 512-byte logical blocks by READ CAPACITY(10), got {bytes}"
            ),
            Self::ScsiAttachment(error) => {
                write!(formatter, "invalid IP12 SCSI attachment: {error}")
            }
        }
    }
}

impl Error for Ip12Error {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ScsiAttachment(error) => Some(error),
            Self::InvalidPromSize { .. }
            | Self::InvalidDiskSize { .. }
            | Self::InvalidCdromSize { .. } => None,
        }
    }
}

/// An IP12 snapshot that cannot preserve configured device topology.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ip12SnapshotError {
    /// The SCSI snapshot differs from the configured targets and storage.
    Scsi(ScsiSnapshotError),
    /// The GIO snapshot differs from the configured device topology.
    Gio(GioSnapshotError),
    /// A snapshot and machine disagree about a configurable port peripheral.
    PortAttachmentMismatch {
        /// Port whose machine-side peripheral presence differs.
        port: Ip12Port,
        /// Whether the snapshot contains the peripheral.
        snapshot_attached: bool,
        /// Whether the target machine contains the peripheral.
        machine_attached: bool,
    },
}

impl fmt::Display for Ip12SnapshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scsi(error) => error.fmt(formatter),
            Self::Gio(error) => error.fmt(formatter),
            Self::PortAttachmentMismatch {
                port,
                snapshot_attached,
                machine_attached,
            } => write!(
                formatter,
                "IP12 snapshot attachment at {port:?} differs: snapshot attached={snapshot_attached}, machine attached={machine_attached}"
            ),
        }
    }
}

impl Error for Ip12SnapshotError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Scsi(error) => Some(error),
            Self::Gio(error) => Some(error),
            Self::PortAttachmentMismatch { .. } => None,
        }
    }
}

impl From<ScsiSnapshotError> for Ip12SnapshotError {
    fn from(error: ScsiSnapshotError) -> Self {
        Self::Scsi(error)
    }
}

impl From<GioSnapshotError> for Ip12SnapshotError {
    fn from(error: GioSnapshotError) -> Self {
        Self::Gio(error)
    }
}

/// Nonvolatile state retained by an Indigo IP12.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
    pub(crate) fn try_receive_input(
        &mut self,
        input: &MachineInput,
    ) -> Result<MachineInputResult, MachineInputError> {
        let endpoint = input.endpoint();
        let consumed = match input.payload() {
            MachineInputPayload::SerialByte(value)
                if endpoint == &Ip12Port::SerialA.endpoint_key() =>
            {
                self.receive_serial_character(Channel::A, *value);
                true
            }
            MachineInputPayload::SerialByte(value)
                if endpoint == &Ip12Port::SerialB.endpoint_key() =>
            {
                self.receive_serial_character(Channel::B, *value);
                true
            }
            MachineInputPayload::Keyboard { key, pressed }
                if endpoint == &Ip12Port::Keyboard.endpoint_key() =>
            {
                let key = translate_keyboard_key(*key)
                    .ok_or(MachineInputError::UnsupportedKeyboardKey)?;
                self.set_sgi_key_state(key, *pressed);
                true
            }
            MachineInputPayload::PointerMotion { delta_x, delta_y }
                if endpoint == &Ip12Port::Mouse.endpoint_key() =>
            {
                self.move_sgi_mouse(*delta_x, *delta_y);
                true
            }
            MachineInputPayload::PointerButton { button, pressed }
                if endpoint == &Ip12Port::Mouse.endpoint_key() =>
            {
                let button = match button {
                    PointerButton::Left => SgiMouseButton::Left,
                    PointerButton::Middle => SgiMouseButton::Middle,
                    PointerButton::Right => SgiMouseButton::Right,
                };
                self.set_sgi_mouse_button_state(button, *pressed);
                true
            }
            MachineInputPayload::EthernetFrame { bytes } if endpoint.as_str() == "ethernet.0" => {
                self.receive_ethernet(bytes)
            }
            _ => unreachable!("IP12 endpoint catalog and input routing must agree"),
        };
        Ok(if consumed {
            MachineInputResult::Consumed
        } else {
            MachineInputResult::WouldBlock
        })
    }

    /// Returns the active machine's frontend I/O endpoints in service order.
    #[must_use]
    pub fn endpoint_catalog(&self) -> EndpointCatalog {
        use EndpointDirection::{Bidirectional, Input, Output};
        use EndpointKind::{Ethernet, Keyboard, Pointer, Serial, Video};

        let mut endpoints = Vec::new();
        if self.bus.has_sgi_keyboard() {
            endpoints.push(EndpointDescriptor::new(
                Ip12Port::Keyboard.endpoint_key(),
                "SGI Keyboard",
                Keyboard,
                Input,
            ));
        }
        if self.bus.has_sgi_mouse() {
            endpoints.push(EndpointDescriptor::new(
                Ip12Port::Mouse.endpoint_key(),
                "SGI Mouse",
                Pointer,
                Input,
            ));
        }
        endpoints.extend([
            EndpointDescriptor::new(
                Ip12Port::SerialA.endpoint_key(),
                "Serial Port A",
                Serial,
                Bidirectional,
            ),
            EndpointDescriptor::new(
                Ip12Port::SerialB.endpoint_key(),
                "Serial Port B",
                Serial,
                Bidirectional,
            ),
            EndpointDescriptor::new(
                EndpointKey::new("ethernet.0"),
                "Ethernet Port 0",
                Ethernet,
                Bidirectional,
            ),
        ]);
        if self.bus.has_video_output() {
            endpoints.push(EndpointDescriptor::new(
                EndpointKey::new("video.0"),
                "LG1 Video Output",
                Video,
                Output,
            ));
        }
        EndpointCatalog::try_new(endpoints).expect("IP12 endpoint identities must be unique")
    }

    /// Constructs an IP12 from a raw U56 PROM dump and optional storage.
    ///
    /// # Errors
    ///
    /// Returns [`Ip12Error`] when the PROM, disk, or CD-ROM image does not
    /// satisfy its IP12 image contract.
    pub fn new(
        raw_prom: Vec<u8>,
        floating_point_backend: Backend,
        gio: GioBus,
        disk_storage: Option<Box<dyn StorageMedium>>,
        cdrom_storage: Option<Box<dyn StorageMedium>>,
    ) -> Result<Self, Ip12Error> {
        Self::new_with_memory(
            raw_prom,
            floating_point_backend,
            Ip12MemoryConfiguration::default(),
            gio,
            disk_storage,
            cdrom_storage,
        )
    }

    /// Constructs an IP12 with explicit boards and optional storage.
    ///
    /// # Errors
    ///
    /// Returns [`Ip12Error`] when the PROM, disk, or CD-ROM image does not
    /// satisfy its IP12 image contract.
    pub fn new_with_memory(
        raw_prom: Vec<u8>,
        floating_point_backend: Backend,
        memory: Ip12MemoryConfiguration,
        gio: GioBus,
        disk_storage: Option<Box<dyn StorageMedium>>,
        cdrom_storage: Option<Box<dyn StorageMedium>>,
    ) -> Result<Self, Ip12Error> {
        validate_u56_prom_size(raw_prom.len())?;
        let mut scsi_bus = ScsiBus::new();
        if let Some(storage) = disk_storage {
            let bytes = storage.size_bytes();
            let target =
                ScsiDisk::try_new(bytes).map_err(|_| Ip12Error::InvalidDiskSize { bytes })?;
            scsi_bus
                .attach(1, 0, Box::new(target), storage)
                .map_err(Ip12Error::ScsiAttachment)?;
        }
        if let Some(storage) = cdrom_storage {
            let bytes = storage.size_bytes();
            let target =
                ScsiCdrom::try_new(bytes).map_err(|_| Ip12Error::InvalidCdromSize { bytes })?;
            scsi_bus
                .attach(4, 0, Box::new(target), storage)
                .map_err(Ip12Error::ScsiAttachment)?;
        }
        Self::new_with_buses(
            raw_prom,
            floating_point_backend,
            memory,
            gio,
            scsi_bus,
            Some(SgiKeyboard::new()),
            Some(SgiMouse::new()),
        )
    }

    fn new_with_buses(
        raw_prom: Vec<u8>,
        floating_point_backend: Backend,
        memory: Ip12MemoryConfiguration,
        gio: GioBus,
        scsi_bus: ScsiBus,
        sgi_keyboard: Option<SgiKeyboard>,
        sgi_mouse: Option<SgiMouse>,
    ) -> Result<Self, Ip12Error> {
        let prom = Rom::new(normalize_u56_prom(raw_prom)?);
        let mut machine = Self {
            cpu: R3000::new(cpu_config(floating_point_backend)),
            bus: Ip12Bus::new(
                Pic1::new(0xf7, 2, true),
                memory_modules(memory),
                Hpc1::new(),
                CentronicsPort::new(),
                Seeq8003::new(),
                Int2::new(),
                Wd33c93b::new(SCSI_CLOCK_HZ),
                scsi_bus,
                [Z85230::new(SERIAL_CLOCK_HZ), Z85230::new(SERIAL_CLOCK_HZ)],
                sgi_keyboard,
                sgi_mouse,
                Dp8573a::new(),
                Mdac::new(),
                Nmc93cs46::new(),
                Dsp56001::new(),
                prom,
                gio,
            ),
        };
        machine.update_cp0_condition();
        Ok(machine)
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
        self.update_cp0_condition();
        self.update_interrupt_lines();
    }

    /// Drives CPCOND high because CPU stores complete synchronously.
    fn update_cp0_condition(&mut self) {
        self.cpu.set_cp0_condition(true);
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

    /// Returns what the machine currently drives onto its display.
    ///
    /// The query has no side effects and does not advance virtual time, so a
    /// paused machine can be asked what to present.
    #[must_use]
    pub(crate) fn video_output(&self) -> Option<VideoOutput> {
        self.bus.video_output()
    }

    pub(crate) fn publish_current_outputs(&self, output: &mut MachineOutput) {
        if let Some(video) = self.video_output() {
            output.publish_video(EndpointKey::new("video.0"), video);
        }
    }

    /// Signals one character arriving at an external serial receiver.
    pub(crate) fn receive_serial_character(&mut self, channel: Channel, value: u8) {
        self.bus.receive_serial_character(channel, value);
        self.update_interrupt_lines();
    }

    /// Applies one physical SGI keyboard key state.
    pub(crate) fn set_sgi_key_state(&mut self, key: SgiKey, pressed: bool) {
        self.bus.set_sgi_key_state(key, pressed);
        self.update_interrupt_lines();
    }

    /// Queues relative SGI mouse motion in guest coordinates.
    pub(crate) fn move_sgi_mouse(&mut self, delta_x: i32, delta_y: i32) {
        self.bus.move_sgi_mouse(delta_x, delta_y);
        self.update_interrupt_lines();
    }

    /// Applies one physical SGI mouse button state.
    pub(crate) fn set_sgi_mouse_button_state(&mut self, button: SgiMouseButton, pressed: bool) {
        self.bus.set_sgi_mouse_button_state(button, pressed);
        self.update_interrupt_lines();
    }

    /// Supplies one external Ethernet frame before device filtering.
    pub(crate) fn receive_ethernet(&mut self, bytes: &[u8]) -> bool {
        self.bus.receive_ethernet(bytes)
    }

    fn update_interrupt_lines(&mut self) {
        let mut interrupt_lines = 0;
        if self.bus.local_interrupt_0_asserted() {
            interrupt_lines |= 1 << 1;
        }
        if self.bus.local_interrupt_1_asserted() {
            interrupt_lines |= 1 << 2;
        }
        if self.bus.timer_0_interrupt_asserted() {
            interrupt_lines |= 1 << 3;
        }
        if self.bus.timer_1_interrupt_asserted() {
            interrupt_lines |= 1 << 4;
        }
        if self.bus.interrupt_asserted() {
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

fn translate_keyboard_key(key: KeyboardKey) -> Option<SgiKey> {
    let code = match key {
        KeyboardKey::Letter(letter @ b'A'..=b'Z') => [
            10, 35, 27, 17, 16, 18, 25, 26, 39, 33, 34, 41, 43, 36, 40, 47, 9, 23, 11, 24, 32, 28,
            15, 20, 31, 19,
        ][usize::from(letter - b'A')],
        KeyboardKey::Digit(digit @ 0..=9) => {
            [45, 7, 13, 14, 21, 22, 29, 30, 37, 38][usize::from(digit)]
        }
        KeyboardKey::KeypadDigit(digit @ 0..=9) => {
            [58, 57, 63, 64, 62, 68, 69, 66, 67, 74][usize::from(digit)]
        }
        KeyboardKey::Function(number @ 1..=12) => 85 + number,
        KeyboardKey::Named(named) => {
            use KeyboardNamedKey as K;
            match named {
                K::LeftControl => 2,
                K::RightControl => 85,
                K::LeftShift => 5,
                K::RightShift => 4,
                K::LeftAlt => 83,
                K::RightAlt => 84,
                K::CapsLock => 3,
                K::Escape => 6,
                K::Tab => 8,
                K::Enter => 50,
                K::Backspace => 60,
                K::Delete => 61,
                K::Space => 82,
                K::ArrowLeft => 72,
                K::ArrowRight => 79,
                K::ArrowUp => 80,
                K::ArrowDown => 73,
                K::Insert => 101,
                K::Home => 102,
                K::End => 104,
                K::PageUp => 103,
                K::PageDown => 105,
                K::PrintScreen => 98,
                K::ScrollLock => 99,
                K::Pause => 100,
                K::NumLock => 106,
                K::Semicolon => 42,
                K::Comma => 44,
                K::Minus => 46,
                K::LeftBracket => 48,
                K::RightBracket => 55,
                K::Apostrophe => 49,
                K::Period => 51,
                K::Slash => 52,
                K::Equal => 53,
                K::Grave => 54,
                K::Backslash => 56,
                K::KeypadPeriod => 65,
                K::KeypadMinus => 75,
                K::KeypadPlus => 109,
                K::KeypadSlash => 107,
                K::KeypadAsterisk => 108,
                K::KeypadEnter => 81,
            }
        }
        _ => return None,
    };
    SgiKey::try_from(code).ok()
}

fn memory_modules(configuration: Ip12MemoryConfiguration) -> [Option<Ram>; 4] {
    let [bank_a, bank_b, bank_c] = configuration.banks();
    [
        bank_a.map(|simm_size| Ram::new(bank_byte_len(simm_size))),
        bank_b.map(|simm_size| Ram::new(bank_byte_len(simm_size))),
        bank_c.map(|simm_size| Ram::new(bank_byte_len(simm_size))),
        None,
    ]
}

fn bank_byte_len(simm_size: Ip12SimmSize) -> usize {
    usize::from(simm_size.simm_mib()) * 4 * 1024 * 1024
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
    use std::io;

    use crate::endpoint::{EndpointDirection, EndpointKey, EndpointKind};
    use crate::input::{KeyboardKey, MachineInput, MachineInputPayload, PointerButton};
    use crate::machine::{Machine, MachineInputError, MachineInputResult};
    use crate::output::{EndpointOutput, MachineOutput, VideoOutput};
    use se_core::bus::{PhysAddr, PhysicalBus};
    use se_core::storage::StorageMedium;
    use se_core::time::{ATTOSECONDS_PER_SECOND, VirtualDuration};
    use se_device::gio::{GioBus, GioSlot};
    use se_device::lg1::Lg1;
    use se_device::z85230::Channel;
    use se_float::backend::Backend;

    use super::{
        CPU_FREQUENCY_HZ, Ip12, Ip12Error, Ip12MemoryConfiguration, Ip12MemoryConfigurationError,
        Ip12NonvolatileState, Ip12NonvolatileStateParts, Ip12Port, Ip12SimmSize, PROM_BYTES,
        RAM_BYTES, cpu_config,
    };

    const MEMORY_CONFIGURATION_INSTRUCTION_BUDGET: usize = 300_000;
    const STACK_SETUP_INSTRUCTION_BUDGET: usize = 30_000;

    #[test]
    fn endpoint_catalog_is_ordered_and_video_requires_graphics() {
        let headless = Ip12::new(
            vec![0; PROM_BYTES],
            Backend::SoftFloat,
            GioBus::new(),
            None,
            None,
        )
        .unwrap();
        let headless_catalog = headless.endpoint_catalog();
        let headless_summary: Vec<_> = headless_catalog
            .endpoints()
            .iter()
            .map(|endpoint| {
                (
                    endpoint.key().as_str(),
                    endpoint.label(),
                    endpoint.kind(),
                    endpoint.direction(),
                )
            })
            .collect();
        assert_eq!(
            headless_summary,
            [
                (
                    "keyboard.0",
                    "SGI Keyboard",
                    EndpointKind::Keyboard,
                    EndpointDirection::Input
                ),
                (
                    "pointer.0",
                    "SGI Mouse",
                    EndpointKind::Pointer,
                    EndpointDirection::Input
                ),
                (
                    "serial.external.a",
                    "Serial Port A",
                    EndpointKind::Serial,
                    EndpointDirection::Bidirectional
                ),
                (
                    "serial.external.b",
                    "Serial Port B",
                    EndpointKind::Serial,
                    EndpointDirection::Bidirectional
                ),
                (
                    "ethernet.0",
                    "Ethernet Port 0",
                    EndpointKind::Ethernet,
                    EndpointDirection::Bidirectional
                ),
            ]
        );
        let mut output = MachineOutput::default();
        headless.publish_current_outputs(&mut output);
        assert!(output.is_empty());

        let mut gio = GioBus::new();
        gio.attach(GioSlot::Graphics, Box::new(Lg1::new())).unwrap();
        let graphics = Ip12::new(vec![0; PROM_BYTES], Backend::SoftFloat, gio, None, None).unwrap();
        let catalog = graphics.endpoint_catalog();
        assert_eq!(&catalog.endpoints()[..5], headless_catalog.endpoints());
        let video = &catalog.endpoints()[5];
        assert_eq!(
            (
                video.key().as_str(),
                video.label(),
                video.kind(),
                video.direction()
            ),
            (
                "video.0",
                "LG1 Video Output",
                EndpointKind::Video,
                EndpointDirection::Output
            )
        );
        let mut output = MachineOutput::default();
        graphics.publish_current_outputs(&mut output);
        assert!(matches!(
            output.entries(),
            [(_, EndpointOutput::Video(VideoOutput::NoSignal))]
        ));
    }

    #[test]
    fn endpoint_input_validates_identity_kind_and_direction() {
        let mut gio = GioBus::new();
        gio.attach(GioSlot::Graphics, Box::new(Lg1::new())).unwrap();
        let mut machine = Machine::IndigoIp12(
            Ip12::new(vec![0; PROM_BYTES], Backend::SoftFloat, gio, None, None).unwrap(),
        );
        let input = |key, payload| MachineInput::new(EndpointKey::new(key), payload);
        assert_eq!(
            machine.try_receive_input(&input("unknown", MachineInputPayload::SerialByte(1))),
            Err(MachineInputError::UnknownEndpoint)
        );
        assert_eq!(
            machine.try_receive_input(&input("video.0", MachineInputPayload::SerialByte(1))),
            Err(MachineInputError::OutputOnlyEndpoint)
        );
        assert_eq!(
            machine.try_receive_input(&input("keyboard.0", MachineInputPayload::SerialByte(1))),
            Err(MachineInputError::PayloadKindMismatch)
        );
        assert_eq!(
            machine.try_receive_input(&input(
                "keyboard.0",
                MachineInputPayload::Keyboard {
                    key: KeyboardKey::Letter(b'a'),
                    pressed: true
                }
            )),
            Err(MachineInputError::UnsupportedKeyboardKey)
        );
        assert_eq!(
            machine.try_receive_input(&input(
                "keyboard.0",
                MachineInputPayload::Keyboard {
                    key: KeyboardKey::Letter(b'A'),
                    pressed: true
                }
            )),
            Ok(MachineInputResult::Consumed)
        );
        assert_eq!(
            machine.try_receive_input(&input(
                "pointer.0",
                MachineInputPayload::PointerMotion {
                    delta_x: 3,
                    delta_y: -2
                }
            )),
            Ok(MachineInputResult::Consumed)
        );
        assert_eq!(
            machine.try_receive_input(&input(
                "pointer.0",
                MachineInputPayload::PointerButton {
                    button: PointerButton::Left,
                    pressed: true
                }
            )),
            Ok(MachineInputResult::Consumed)
        );
    }

    #[test]
    fn external_serial_endpoints_route_to_distinct_scc_channels() {
        const SERIAL_BASE: u64 = 0x1fb8_0d10;
        let mut ip12 = Ip12::new(
            vec![0; PROM_BYTES],
            Backend::SoftFloat,
            GioBus::new(),
            None,
            None,
        )
        .unwrap();
        for control in [0x0b, 0x03] {
            ip12.bus
                .write(PhysAddr::new(SERIAL_BASE + control), &[3])
                .unwrap();
            ip12.bus
                .write(PhysAddr::new(SERIAL_BASE + control), &[1])
                .unwrap();
        }
        let mut machine = Machine::IndigoIp12(ip12);
        for (key, value) in [("serial.external.a", b'A'), ("serial.external.b", b'B')] {
            assert_eq!(
                machine.try_receive_input(&MachineInput::new(
                    EndpointKey::new(key),
                    MachineInputPayload::SerialByte(value)
                )),
                Ok(MachineInputResult::Consumed)
            );
        }
        let Machine::IndigoIp12(mut ip12) = machine;
        for (data, expected) in [(0x0f, b'A'), (0x07, b'B')] {
            let mut received = [0];
            ip12.bus
                .read(PhysAddr::new(SERIAL_BASE + data), &mut received)
                .unwrap();
            assert_eq!(received, [expected]);
        }
    }

    #[test]
    fn external_serial_arrivals_are_consumed_when_the_receive_fifo_overruns() {
        const SERIAL_BASE: u64 = 0x1fb8_0d10;
        let mut ip12 = Ip12::new(
            vec![0; PROM_BYTES],
            Backend::SoftFloat,
            GioBus::new(),
            None,
            None,
        )
        .unwrap();
        ip12.bus
            .write(PhysAddr::new(SERIAL_BASE + 0x0b), &[3])
            .unwrap();
        ip12.bus
            .write(PhysAddr::new(SERIAL_BASE + 0x0b), &[1])
            .unwrap();
        let mut machine = Machine::IndigoIp12(ip12);
        let endpoint = Ip12Port::SerialA.endpoint_key();

        for value in 0..10 {
            assert_eq!(
                machine.try_receive_input(&MachineInput::new(
                    endpoint.clone(),
                    MachineInputPayload::SerialByte(value)
                )),
                Ok(MachineInputResult::Consumed)
            );
        }

        let Machine::IndigoIp12(mut ip12) = machine;
        for expected in [0, 1, 2, 3, 4, 5, 6, 9] {
            let mut received = [0];
            ip12.bus
                .read(PhysAddr::new(SERIAL_BASE + 0x0f), &mut received)
                .unwrap();
            assert_eq!(received, [expected]);
        }
    }

    #[test]
    fn keyboard_and_pointer_endpoints_reach_scc_zero_protocol_channels() {
        const SERIAL_BASE: u64 = 0x1fb8_0d00;
        let mut ip12 = Ip12::new(
            vec![0; PROM_BYTES],
            Backend::SoftFloat,
            GioBus::new(),
            None,
            None,
        )
        .unwrap();
        for control in [0x0b, 0x03] {
            ip12.bus
                .write(PhysAddr::new(SERIAL_BASE + control), &[3])
                .unwrap();
            ip12.bus
                .write(PhysAddr::new(SERIAL_BASE + control), &[1])
                .unwrap();
        }
        let mut machine = Machine::IndigoIp12(ip12);
        assert_eq!(
            machine.try_receive_input(&MachineInput::new(
                EndpointKey::new("keyboard.0"),
                MachineInputPayload::Keyboard {
                    key: KeyboardKey::Letter(b'A'),
                    pressed: true
                }
            )),
            Ok(MachineInputResult::Consumed)
        );
        assert_eq!(
            machine.try_receive_input(&MachineInput::new(
                EndpointKey::new("pointer.0"),
                MachineInputPayload::PointerButton {
                    button: PointerButton::Left,
                    pressed: true
                }
            )),
            Ok(MachineInputResult::Consumed)
        );
        machine.advance_time(
            VirtualDuration::from_attoseconds(20 * ATTOSECONDS_PER_SECOND / 1000),
            &mut MachineOutput::default(),
        );
        let Machine::IndigoIp12(mut ip12) = machine;
        for (data, expected) in [(0x0f, 10), (0x07, 0x83)] {
            let mut received = [0];
            ip12.bus
                .read(PhysAddr::new(SERIAL_BASE + data), &mut received)
                .unwrap();
            assert_eq!(received, [expected]);
        }
    }

    struct SizedStorage(u64);

    impl StorageMedium for SizedStorage {
        fn size_bytes(&self) -> u64 {
            self.0
        }

        fn read_exact_at(&mut self, _offset: u64, buffer: &mut [u8]) -> io::Result<()> {
            buffer.fill(0);
            Ok(())
        }

        fn write_all_at(&mut self, _offset: u64, _data: &[u8]) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn memory_configuration_accepts_every_supported_nonempty_bank_combination() {
        let capacities = [0, 2, 4, 8];
        let mut accepted = 0;
        for bank_a in capacities {
            for bank_b in capacities {
                for bank_c in capacities {
                    if [bank_a, bank_b, bank_c] == [0, 0, 0] {
                        continue;
                    }
                    let configuration =
                        Ip12MemoryConfiguration::try_from_simm_mib([bank_a, bank_b, bank_c])
                            .unwrap();
                    assert_eq!(configuration.simm_mib(), [bank_a, bank_b, bank_c]);
                    accepted += 1;
                }
            }
        }
        assert_eq!(accepted, 63);

        let configuration = Ip12MemoryConfiguration::try_from_simm_mib([0, 4, 8]).unwrap();

        assert_eq!(
            configuration.banks(),
            [None, Some(Ip12SimmSize::Mib4), Some(Ip12SimmSize::Mib8),]
        );
    }

    #[test]
    fn memory_configuration_rejects_empty_and_unsupported_banks() {
        assert_eq!(
            Ip12MemoryConfiguration::try_from_simm_mib([0, 0, 0]),
            Err(Ip12MemoryConfigurationError::NoInstalledBank)
        );
        assert_eq!(
            Ip12MemoryConfiguration::try_from_simm_mib([2, 16, 8]),
            Err(Ip12MemoryConfigurationError::UnsupportedSimmCapacity { bank: 1, mib: 16 })
        );
    }

    #[test]
    fn constructor_reports_invalid_prom_size() {
        assert!(matches!(
            Ip12::new(
                vec![0; PROM_BYTES - 1],
                Backend::SoftFloat,
                GioBus::new(),
                None,
                None,
            ),
            Err(Ip12Error::InvalidPromSize {
                expected: PROM_BYTES,
                actual
            }) if actual == PROM_BYTES - 1
        ));
    }

    #[test]
    fn legacy_constructor_prioritizes_prom_error_over_disk_error() {
        assert!(matches!(
            Ip12::new_with_memory(
                vec![0; PROM_BYTES - 1],
                Backend::SoftFloat,
                Ip12MemoryConfiguration::default(),
                GioBus::new(),
                Some(Box::new(SizedStorage(513))),
                None,
            ),
            Err(Ip12Error::InvalidPromSize {
                expected: PROM_BYTES,
                actual
            }) if actual == PROM_BYTES - 1
        ));
    }

    #[test]
    fn constructor_validates_optional_disk_capacity() {
        for bytes in [0, 513, (u64::from(u32::MAX) + 2) * 512] {
            assert!(matches!(
                Ip12::new(
                    vec![0; PROM_BYTES],
                    Backend::SoftFloat,
                    GioBus::new(),
                    Some(Box::new(SizedStorage(bytes))),
                    None,
                ),
                Err(Ip12Error::InvalidDiskSize { bytes: actual }) if actual == bytes
            ));
        }

        assert!(
            Ip12::new(
                vec![0; PROM_BYTES],
                Backend::SoftFloat,
                GioBus::new(),
                Some(Box::new(SizedStorage(512))),
                None,
            )
            .is_ok()
        );
    }

    #[test]
    fn constructor_validates_optional_cdrom_capacity() {
        for bytes in [0, 512, 2049, (u64::from(u32::MAX) + 2) * 512] {
            assert!(matches!(
                Ip12::new(
                    vec![0; PROM_BYTES],
                    Backend::SoftFloat,
                    GioBus::new(),
                    None,
                    Some(Box::new(SizedStorage(bytes))),
                ),
                Err(Ip12Error::InvalidCdromSize { bytes: actual }) if actual == bytes
            ));
        }

        assert!(
            Ip12::new(
                vec![0; PROM_BYTES],
                Backend::SoftFloat,
                GioBus::new(),
                None,
                Some(Box::new(SizedStorage(2048))),
            )
            .is_ok()
        );
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
    fn cold_start_drives_cp0_condition_for_both_branch_polarities() {
        let mut machine = machine_with_cp0_branches();
        assert_cp0_condition_branches(&mut machine);
    }

    #[test]
    fn reset_reasserts_cp0_condition() {
        let mut machine = machine_with_cp0_branches();
        machine.cpu.set_cp0_condition(false);
        machine.reset();
        assert_cp0_condition_branches(&mut machine);
    }

    #[test]
    fn snapshot_restore_reasserts_cp0_condition_without_changing_pending_branch() {
        let mut machine = machine_with_cp0_branches();
        machine.cpu.set_cp0_condition(false);
        machine.execute_instruction().unwrap();
        assert_eq!(machine.execution_address(), 0xbfc0_0004);
        let snapshot = machine.snapshot().unwrap();

        machine.execute_instruction().unwrap();
        machine.restore_snapshot(snapshot).unwrap();
        assert_eq!(machine.execution_address(), 0xbfc0_0004);
        assert_eq!(machine.cpu.debug_snapshot().gpr[8], 0);

        // Restoring the input must not change a previously selected target.
        machine.execute_instruction().unwrap();
        assert_eq!(machine.execution_address(), 0xbfc0_0000);
        assert_eq!(machine.cpu.debug_snapshot().gpr[8], 1);
        assert_cp0_condition_branches(&mut machine);
    }

    fn machine_with_cp0_branches() -> Ip12 {
        machine_with_instructions(&[
            0x4100_ffff, // BC0F to itself.
            0x2508_0001, // ADDIU t0, t0, 1 in the delay slot.
            0x4101_0002, // BC0T skips the fall-through marker.
            0x2508_0001, // ADDIU t0, t0, 1 in the delay slot.
            0x2409_0001, // ADDIU t1, zero, 1 on fall-through.
            0x240a_0001, // ADDIU t2, zero, 1 at the branch target.
        ])
    }

    fn assert_cp0_condition_branches(machine: &mut Ip12) {
        let initial_delay_slots = machine.cpu.debug_snapshot().gpr[8];
        machine.execute_instruction().unwrap();
        assert_eq!(machine.execution_address(), 0xbfc0_0004);
        machine.execute_instruction().unwrap();
        assert_eq!(machine.execution_address(), 0xbfc0_0008);
        machine.execute_instruction().unwrap();
        assert_eq!(machine.execution_address(), 0xbfc0_000c);
        machine.execute_instruction().unwrap();
        assert_eq!(machine.execution_address(), 0xbfc0_0014);
        machine.execute_instruction().unwrap();

        let state = machine.cpu.debug_snapshot();
        assert_eq!(state.gpr[8], initial_delay_slots + 2);
        assert_eq!(state.gpr[9], 0);
        assert_eq!(state.gpr[10], 1);
    }

    #[test]
    fn cpu_configuration_matches_the_ip12_board() {
        for backend in [Backend::SoftFloat, Backend::Native] {
            let config = cpu_config(backend);
            let mut machine =
                Ip12::new(vec![0; PROM_BYTES], backend, GioBus::new(), None, None).unwrap();
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
        let mut machine = Ip12::new(
            vec![0; PROM_BYTES],
            Backend::SoftFloat,
            GioBus::new(),
            None,
            None,
        )
        .unwrap();
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
    fn explicit_memory_configuration_populates_selected_pic1_banks() {
        let memory = Ip12MemoryConfiguration::try_from_simm_mib([2, 0, 8]).unwrap();
        let mut machine = Ip12::new_with_memory(
            vec![0; PROM_BYTES],
            Backend::SoftFloat,
            memory,
            GioBus::new(),
            None,
            None,
        )
        .unwrap();
        machine
            .bus
            .write(PhysAddr::new(0x1fa1_0000), &0x0100_023f_u32.to_be_bytes())
            .unwrap();
        machine
            .bus
            .write(PhysAddr::new(0x1fa1_0004), &0x0702_023f_u32.to_be_bytes())
            .unwrap();

        machine
            .bus
            .write(
                PhysAddr::new(8 * 1024 * 1024 - 4),
                &0x0123_4567_u32.to_be_bytes(),
            )
            .unwrap();
        machine
            .bus
            .write(
                PhysAddr::new(40 * 1024 * 1024 - 4),
                &0x89ab_cdef_u32.to_be_bytes(),
            )
            .unwrap();

        assert_eq!(read_word(&mut machine, 8 * 1024 * 1024 - 4), 0x0123_4567);
        assert_eq!(read_word(&mut machine, 40 * 1024 * 1024 - 4), 0x89ab_cdef);
    }

    #[test]
    fn snapshot_restore_repeats_machine_execution_and_output() {
        let mut machine = machine_with_instructions(&[0x2408_0001, 0x2508_0001, 0]);
        machine
            .bus
            .write(PhysAddr::new(0x1fa1_0000), &0x0f00_023f_u32.to_be_bytes())
            .unwrap();
        machine
            .bus
            .write(PhysAddr::new(0x1000), &0x1234_5678_u32.to_be_bytes())
            .unwrap();
        machine.execute_instruction().unwrap();
        let mut initial_output = MachineOutput::default();
        machine.advance_time(
            VirtualDuration::from_attoseconds(123_456),
            &mut initial_output,
        );
        let snapshot = machine.snapshot().unwrap();
        let expected_fingerprint = machine.machine_state_fingerprint();
        let expected_address = machine.execution_address();
        let expected_ram = read_word(&mut machine, 0x1000);

        machine.execute_instruction().unwrap();
        let mut first_output = MachineOutput::default();
        machine.advance_time(
            VirtualDuration::from_attoseconds(987_654),
            &mut first_output,
        );
        let first_result = (
            machine.machine_state_fingerprint(),
            machine.execution_address(),
            read_word(&mut machine, 0x1000),
            first_output,
        );

        machine.restore_snapshot(snapshot).unwrap();
        assert_eq!(machine.machine_state_fingerprint(), expected_fingerprint);
        assert_eq!(machine.execution_address(), expected_address);
        assert_eq!(read_word(&mut machine, 0x1000), expected_ram);
        machine.execute_instruction().unwrap();
        let mut second_output = MachineOutput::default();
        machine.advance_time(
            VirtualDuration::from_attoseconds(987_654),
            &mut second_output,
        );
        let second_result = (
            machine.machine_state_fingerprint(),
            machine.execution_address(),
            read_word(&mut machine, 0x1000),
            second_output,
        );

        assert_eq!(second_result, first_result);
    }

    #[test]
    fn reset_restores_the_cpu_and_asic_front_end_without_changing_ram_or_prom() {
        let mut raw_prom = vec![0; PROM_BYTES];
        raw_prom[0x100..0x104].copy_from_slice(&[0x34, 0x12, 0x78, 0x56]);
        let mut machine =
            Ip12::new(raw_prom, Backend::SoftFloat, GioBus::new(), None, None).unwrap();

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
        assert!(machine.bus.interrupt_asserted());
        advance_machine_interrupt_inputs(&mut machine);
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
        assert!(!machine.bus.interrupt_asserted());
        assert_ne!(
            machine.cpu.debug_snapshot().cp0.registers[13] & (1 << 15),
            0
        );
        advance_machine_interrupt_inputs(&mut machine);
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
        let mut machine = Ip12::new(
            vec![0; PROM_BYTES],
            Backend::SoftFloat,
            GioBus::new(),
            None,
            None,
        )
        .unwrap();
        machine
            .bus
            .write(
                PhysAddr::new(4 * 1024 * 1024),
                &0x0123_4567_u32.to_be_bytes(),
            )
            .unwrap();

        advance_machine_interrupt_inputs(&mut machine);
        assert_ne!(
            machine.cpu.debug_snapshot().cp0.registers[13] & (1 << 15),
            0
        );

        machine.bus.write(PhysAddr::new(0x1fa1_0210), &[0]).unwrap();
        advance_machine_interrupt_inputs(&mut machine);
        assert_eq!(
            machine.cpu.debug_snapshot().cp0.registers[13] & (1 << 15),
            0
        );
    }

    #[test]
    fn guest_store_error_crosses_two_interrupt_input_boundaries() {
        let mut machine = machine_with_instructions(&[0x3c08_a040, 0xad00_0000, 0]);

        machine.execute_instruction().unwrap();
        machine.execute_instruction().unwrap();
        assert!(machine.bus.interrupt_asserted());
        assert_eq!(
            machine.cpu.debug_snapshot().cp0.registers[13] & (1 << 15),
            0
        );

        machine.execute_instruction().unwrap();
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
    fn guest_can_poll_and_clear_optional_controller_errors_with_interrupts_disabled() {
        for physical_address in [0x1fb0_0010_u32, 0x1fb0_0050, 0x1f98_0010, 0x1f98_0050] {
            let virtual_address = physical_address | 0xa000_0000;
            let mut instructions = vec![
                0x3c08_bfa1,                              // lui t0, 0xbfa1
                0x3c09_0000 | (virtual_address >> 16),    // lui t1, target high
                0x3529_0000 | (virtual_address & 0xffff), // ori t1, t1, target low
                0x3c0a_bfa0,                              // lui t2, 0xbfa0
                0x340b_aaaa,                              // ori t3, zero, 0xaaaa
                0x4080_6000,                              // mtc0 zero, Status
                0,
                0,
            ];
            let probe = [
                0xad00_0210, // sw zero, CLERERR(t0)
                0xad2b_0000, // sw t3, 0(t1)
                0x8d40_0000, // lw zero, CPUCTRL(t2)
                0x8d40_0000, // lw zero, CPUCTRL(t2)
                0x400c_6800, // mfc0 t4, Cause
                0,
                0x318c_8000, // andi t4, t4, 0x8000
                0xad00_0210, // sw zero, CLERERR(t0)
                0,
                0,
                0x400d_6800, // mfc0 t5, Cause
                0,
                0x31ad_8000, // andi t5, t5, 0x8000
            ];
            instructions.extend(probe);
            instructions.extend(probe);
            let mut machine = machine_with_instructions(&instructions);

            for _ in 0..8 {
                machine.execute_instruction().unwrap();
            }
            for _ in 0..2 {
                for _ in &probe {
                    machine.execute_instruction().unwrap();
                }
                let state = machine.cpu.debug_snapshot();
                assert_eq!(state.gpr[12], 0x8000);
                assert_eq!(state.gpr[13], 0);
                assert_eq!(state.cp0.registers[12] & 1, 0);
                assert!(!machine.bus.interrupt_asserted());
            }
            assert_eq!(
                machine.execution_address(),
                0xbfc0_0000 + u32::try_from(instructions.len() * 4).unwrap()
            );
        }
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
        assert_ne!(
            machine.cpu.debug_snapshot().cp0.registers[13] & (1 << 10),
            0
        );
    }

    #[test]
    fn serial_receive_interrupt_drives_cpu_interrupt_input_one() {
        let mut machine = Ip12::new(
            vec![0; PROM_BYTES],
            Backend::SoftFloat,
            GioBus::new(),
            None,
            None,
        )
        .unwrap();
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

        machine.receive_serial_character(Channel::A, b'A');
        advance_machine_interrupt_inputs(&mut machine);
        assert_ne!(
            machine.cpu.debug_snapshot().cp0.registers[13] & (1 << 11),
            0
        );
    }

    #[test]
    fn hpc1_interrupt_outputs_drive_cpu_inputs_one_and_two() {
        let mut machine = Ip12::new(
            vec![0; PROM_BYTES],
            Backend::SoftFloat,
            GioBus::new(),
            None,
            None,
        )
        .unwrap();
        machine
            .bus
            .write(PhysAddr::new(0x1fb8_01c7), &[1 << 1])
            .unwrap();
        machine
            .bus
            .write(PhysAddr::new(0x1fb8_01cf), &[1 << 4])
            .unwrap();

        machine.bus.set_hpc1_interrupt_levels_for_test(true, true);
        machine.update_interrupt_lines();
        advance_machine_interrupt_inputs(&mut machine);
        assert_eq!(
            machine.cpu.debug_snapshot().cp0.registers[13] & 0x0000_1800,
            0x0000_1800
        );

        machine.bus.set_hpc1_interrupt_levels_for_test(false, false);
        machine.update_interrupt_lines();
        advance_machine_interrupt_inputs(&mut machine);
        assert_eq!(
            machine.cpu.debug_snapshot().cp0.registers[13] & 0x0000_1800,
            0
        );
    }

    #[test]
    fn int2_timers_drive_cpu_interrupt_inputs_three_and_four() {
        let mut machine = Ip12::new(
            vec![0; PROM_BYTES],
            Backend::SoftFloat,
            GioBus::new(),
            None,
            None,
        )
        .unwrap();
        for (control, address) in [
            (0xb4, 0x1fb8_01fb),
            (0x34, 0x1fb8_01f3),
            (0x74, 0x1fb8_01f7),
        ] {
            machine
                .bus
                .write(PhysAddr::new(0x1fb8_01ff), &[control])
                .unwrap();
            for value in 2_u16.to_le_bytes() {
                machine.bus.write(PhysAddr::new(address), &[value]).unwrap();
            }
        }
        let mut output = MachineOutput::default();

        machine.advance_time(
            VirtualDuration::from_attoseconds(6_000_000_000_000),
            &mut output,
        );
        machine.update_interrupt_lines();
        advance_machine_interrupt_inputs(&mut machine);
        assert_eq!(
            machine.cpu.debug_snapshot().cp0.registers[13] & 0x0000_6000,
            0x0000_6000
        );

        machine.bus.write(PhysAddr::new(0x1fb8_01e3), &[1]).unwrap();
        machine.update_interrupt_lines();
        advance_machine_interrupt_inputs(&mut machine);
        assert_eq!(
            machine.cpu.debug_snapshot().cp0.registers[13] & 0x0000_6000,
            0x0000_4000
        );

        machine.bus.write(PhysAddr::new(0x1fb8_01e3), &[2]).unwrap();
        machine.update_interrupt_lines();
        advance_machine_interrupt_inputs(&mut machine);
        assert_eq!(
            machine.cpu.debug_snapshot().cp0.registers[13] & 0x0000_6000,
            0
        );
    }

    #[test]
    fn guest_system_initialize_uses_the_machine_reset_path() {
        let mut machine = Ip12::new(
            vec![0; PROM_BYTES],
            Backend::SoftFloat,
            GioBus::new(),
            None,
            None,
        )
        .unwrap();
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

    fn advance_machine_interrupt_inputs(machine: &mut Ip12) {
        machine.execute_instruction().unwrap();
        machine.execute_instruction().unwrap();
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
        Ip12::new(raw_prom, Backend::SoftFloat, GioBus::new(), None, None).unwrap()
    }

    #[test]
    #[ignore = "requires an external 070-8088-002 IP12 PROM dump"]
    fn reset_vector_reaches_reset_entry() {
        let path = env::var_os("SE_INDIGO_IP12_PROM")
            .expect("SE_INDIGO_IP12_PROM must name the external PROM dump");
        let raw_prom = fs::read(path).expect("the external PROM dump should be readable");
        let mut machine = Ip12::new(raw_prom, Backend::SoftFloat, GioBus::new(), None, None)
            .expect("the PROM dump should be valid");

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
        let mut machine = Ip12::new(raw_prom, Backend::SoftFloat, GioBus::new(), None, None)
            .expect("the PROM dump should be valid");

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
        let mut machine = Ip12::new(raw_prom, Backend::SoftFloat, GioBus::new(), None, None)
            .expect("the PROM dump should be valid");

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
        let mut machine = Ip12::new(raw_prom, Backend::SoftFloat, GioBus::new(), None, None)
            .expect("the PROM dump should be valid");

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
        assert!(!machine.bus.interrupt_asserted());

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
        assert!(!machine.bus.interrupt_asserted());
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
