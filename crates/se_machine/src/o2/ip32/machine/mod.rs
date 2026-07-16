//! Runtime integration for the SGI O2 IP32 machine profile.

use core::fmt;
use std::collections::VecDeque;
#[cfg(feature = "jit")]
use std::collections::{HashMap, HashSet};
#[cfg(feature = "jit")]
use std::hash::{BuildHasherDefault, Hasher};

use se_core::component::{Component, ComponentId, ComponentStateError};
use se_core::role::{BusControllerRole, BusDeviceRole, BusRole};
#[cfg(feature = "jit")]
use se_core::scheduler::FractionalClockProjection;
use se_core::scheduler::{ScheduledEvent, ScheduledEventId, SchedulerError, SimDuration, SimTime};
use se_core::tracing::{
    NoopTraceSink, TraceField, TraceInterest, TraceLevel, TraceSink, TraceSource,
};
use se_device::bus::i2c::{I2cBus, I2cBusAction, I2cCompletion};
use se_device::bus::irq::{
    IrqBus, IrqBusAction, IrqBusBuildError, IrqBusRouteError, IrqRoute, IrqSource, IrqTarget,
};
use se_device::bus::isa::{
    IsaBus, IsaBusAction, IsaBusDisposition, IsaCompletion, IsaDeviceResponse, IsaTransaction,
};
use se_device::bus::media::{MediaBus, MediaBusAction, MediaPayload, MediaPort, MediaTransaction};
use se_device::bus::one_wire::{
    OneWireBus, OneWireBusAction, OneWireBusBuildError, OneWireBusRouteError,
};
use se_device::bus::pci::{
    PciBus, PciBusAction, PciCompletion, PciConfigurationEndpoint, PciStatus,
};
use se_device::bus::two_wire::{
    TwoWireBus, TwoWireBusAction, TwoWireBusBuildError, TwoWireBusRouteError,
};
#[cfg(feature = "jit")]
use se_device::chipset::crime::CrimeSynchronousReadSnapshot;
use se_device::chipset::crime::config::CrimeConfig;
use se_device::chipset::crime::iou::{
    CrimeCgiBus, CrimeCgiBusEvent, CrimeCmiBus, CrimeCmiBusEvent,
};
use se_device::chipset::crime::memory::CrimeSdram;
#[cfg(feature = "jit")]
use se_device::chipset::crime::memory::bus::CrimeDirectMemoryPlan;
use se_device::chipset::crime::memory::bus::{CrimeMemoryBus, CrimeMemoryBusEvent};
#[cfg(feature = "jit")]
use se_device::chipset::crime::protocol::CrimeSysAdRoute;
#[cfg(feature = "jit")]
use se_device::chipset::crime::protocol::CrimeTransferView;
use se_device::chipset::crime::protocol::{
    CRIME_IRQ_OUTPUT, CrimeAction, CrimeBusAction, CrimeBusDisposition, CrimeCgiTransaction,
    CrimeCmiTransaction, CrimeCpuSignal, CrimeLinkDeviceResponse, CrimeMemoryTransaction,
    CrimePoll, CrimeSysAdRequest, CrimeTraceEvent, CrimeTraceValue,
};
use se_device::chipset::crime::{Crime, CrimeError};
use se_device::chipset::gbe::Gbe;
use se_device::chipset::gbe::protocol::{
    GbeAction, GbeExternalInput, GbeFrame, GbeOutputPins, GbePoll, GbeTraceEvent, GbeTraceLevel,
    GbeTraceValue, GbeWiring,
};
use se_device::chipset::mace::config::{MaceConfig, MacePortConfig};
use se_device::chipset::mace::protocol::{
    MaceAction, MaceExternalLinks, MacePoll, MaceTraceEvent, MaceTraceValue, MaceWiring,
};
use se_device::chipset::mace::{
    MACE_IRQ_PARALLEL, MACE_IRQ_RTC, MACE_IRQ_SERIAL0, MACE_IRQ_SERIAL1, Mace, MaceError,
};
use se_device::cpu::execution::protocol::{
    ExecutionAction, ExecutionCompletion, ExecutionTransaction,
};
use se_device::cpu::mips4::config::{Mips4CacheConfig, Mips4Endianness};
#[cfg(feature = "jit")]
use se_device::cpu::mips4::execution::block::{
    Mips4FastMemoryReadRequest, Mips4FastMemoryReadResult, Mips4FastMemoryRuntime,
};
#[cfg(feature = "jit")]
use se_device::cpu::mips4::execution::bus::{Mips4ExecutionAccessKind, Mips4ExecutionTransferSize};
use se_device::cpu::mips4::execution::bus::{Mips4ExecutionCompletion, Mips4ExecutionTransaction};
use se_device::cpu::mips4::execution::target::Mips4ExecutionBoundary;
use se_device::cpu::mips4::model::r5000::boot_mode::R5000BootMode;
use se_device::cpu::mips4::model::r5000::cpu::{
    R5000_IRQ_IP2, R5000Cpu, R5000CpuError, R5000CpuSignal, R5000CpuStatistics, R5000IrqError,
};
use se_device::cpu::mips4::model::r5000::profile::R5000Profile;
use se_device::cpu::mips4::model::r5000::revision::R5000Revision;
use se_device::input::ps2::{
    Ps2DeviceBuildError, Ps2Keyboard, Ps2KeyboardAction, Ps2KeyboardError, Ps2KeyboardPoll,
    Ps2Mouse, Ps2MouseAction, Ps2MouseError, Ps2MousePoll, Ps2Wiring,
};
use se_device::memory::ds2502::{Ds2502, Ds2502Action, Ds2502Config, Ds2502Error};
use se_device::memory::flash::{SystemFlash, SystemFlashPersistentState, SystemFlashStateError};
use se_device::parallel::ieee1284::{IEEE1284_IRQ_OUTPUT, Ieee1284, Ieee1284Action};
use se_device::rtc::ds1687::state::{Ds1687PersistentState, Ds1687StateError};
use se_device::rtc::ds1687::{DS1687_IRQ_OUTPUT, Ds1687, Ds1687Action, Ds1687Config, Ds1687Error};
use se_device::serial::uart16550::{
    UART16550_IRQ_OUTPUT, Uart16550, Uart16550Action, Uart16550Config, Uart16550Error,
};
use se_runtime::registry::{ComponentRegistry, ComponentSlot, RegistryError, RegistryLookupError};
use se_runtime::runtime::event_chain::EventChainError;
use se_runtime::runtime::{RunError, RunStatus, Runtime, RuntimeContext, RuntimeStatistics};

use super::address_map::IP32_PROM_IMAGE_SIZE_BYTES;
#[cfg(feature = "jit")]
use super::address_map::{Ip32AddressResolution, Ip32PhysicalRegion};
#[cfg(feature = "jit")]
use super::bus::Ip32DirectSysAdPlan;
use super::bus::{Ip32StubEndpoint, Ip32SysAdBus, Ip32SysAdBusAction};
use super::component_ids;
#[cfg(test)]
use super::dispatch::LogicalTransition;
use super::dispatch::{Ip32DispatchContext, Ip32EventChainPolicy};
use super::event::{
    Ip32Event, Ip32HostInput, Ip32HostIoStats, Ip32HostOutput, Ip32InputEvent, Ip32SerialOutput,
    Ip32SerialPort,
};
use super::state::{
    IP32_STATE_SCHEMA_VERSION, Ip32MachineState, Ip32PersistentConfig, Ip32StateError,
    MachineControlState,
};
#[cfg(feature = "jit")]
use se_device::cpu::mips4::execution::block::{
    MIPS4_BLOCK_MAX_INSTRUCTIONS, Mips4CodeGuard, Mips4CodeSourceId, Mips4CodeWindow,
    Mips4SliceClock, Mips4SliceTimeline,
};
#[cfg(feature = "jit")]
use se_device::cpu::mips4::model::r5000::cpu::R5000ExecutionSliceAction;
#[cfg(feature = "jit")]
use se_jit::mips4::cranelift::CraneliftMips4Backend;
#[cfg(feature = "jit")]
use se_jit::mips4::engine::{MIPS4_BLOCK_CACHE_CAPACITY, Mips4BlockEngine};

#[cfg(feature = "jit")]
mod fast_memory;
use super::timing::IP32_TIMEBASE_HZ;
#[cfg(feature = "jit")]
use fast_memory::{
    commit_fast_memory_runtime, configure_fast_memory_code_timeline, plan_fast_memory_runtime,
};

const DEFAULT_PROCESSOR_FREQUENCY_HZ: u64 = 180_000_000;
const PRIMARY_CACHE_SIZE_BYTES: u32 = 32 * 1024;
const SECONDARY_CACHE_SIZE_BYTES: u32 = 512 * 1024;
const CACHE_LINE_SIZE_BYTES: u32 = 32;
const SECONDARY_CACHE_ENABLE_BIT: u64 = 1 << 12;
const SDRAM_REFRESH_TICKS: u64 = 27_000;
const SDRAM_INITIALIZATION_TICKS: u64 = 120_000;
const ISA_CYCLE_TICKS: u64 = 1_000;
const PCI_CYCLE_TICKS: u64 = 30;
const UART_INPUT_CLOCK_HZ: u64 = 22_000_000;
const DEFAULT_CPU_CONTINUATION_QUANTUM: usize = 256;

/// Complete construction input for one IP32 machine.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Ip32MachineConfig {
    /// Enables the host-native tiered execution engine.
    #[serde(default)]
    pub jit_enabled: bool,

    /// R5000 processor identity, byte order, clocks, and cache geometry.
    pub processor: R5000Profile,

    /// R5000 boot-mode serial stream sampled at reset.
    pub boot_mode: R5000BootMode,

    /// CRIME chipset and physical SDRAM topology.
    pub crime: CrimeConfig,

    /// MACE I/O ASIC configuration.
    pub mace: MaceConfig,

    /// Deterministic DS1687 RTC time and battery-backed register/NVRAM image.
    ///
    /// The IP32 PROM environment is not stored in this device.
    pub rtc: Ds1687Config,

    /// Deterministic board-identity ROM and EPROM image.
    pub nic_identity: Ds2502Config,

    /// Immutable base image for the 512 KiB byte-programmable System Flash.
    ///
    /// On IP32, the PROM environment resides in this System Flash and remains
    /// physically separate from the DS1687 RTC/NVRAM domain.
    pub prom_image: Vec<u8>,
}

impl Default for Ip32MachineConfig {
    fn default() -> Self {
        Self {
            jit_enabled: false,
            processor: R5000Profile::new(
                Mips4Endianness::Big,
                R5000Revision::from_bits(0x21),
                DEFAULT_PROCESSOR_FREQUENCY_HZ,
                Mips4CacheConfig::present(PRIMARY_CACHE_SIZE_BYTES, CACHE_LINE_SIZE_BYTES),
                Mips4CacheConfig::present(PRIMARY_CACHE_SIZE_BYTES, CACHE_LINE_SIZE_BYTES),
                Mips4CacheConfig::present(SECONDARY_CACHE_SIZE_BYTES, CACHE_LINE_SIZE_BYTES),
            ),
            boot_mode: R5000BootMode::from_low_bits(SECONDARY_CACHE_ENABLE_BIT)
                .expect("the default R5000 boot mode must be valid"),
            crime: CrimeConfig::default(),
            mace: MaceConfig::default(),
            rtc: Ds1687Config::default(),
            nic_identity: Ds2502Config::default(),
            prom_image: vec![0; IP32_PROM_IMAGE_SIZE_BYTES],
        }
    }
}

/// Error returned while constructing an IP32 machine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Ip32MachineBuildError {
    /// JIT execution was requested from a build without JIT support.
    JitUnavailable,

    /// The host-native backend could not be initialized.
    JitInitialization(String),

    /// CRIME configuration is invalid.
    Crime(CrimeError),

    /// MACE configuration is invalid.
    Mace(MaceError),

    /// A PS/2 input device configuration is invalid.
    Ps2(Ps2DeviceBuildError),

    /// RTC configuration is invalid.
    Rtc(Ds1687Error),

    /// Board-identity memory configuration is invalid.
    NicIdentity(Ds2502Error),

    /// A UART configuration is invalid.
    Uart(Uart16550Error),

    /// The CPU interrupt routing table is invalid.
    IrqBus(IrqBusBuildError),

    /// The board-identity 1-Wire topology is invalid.
    OneWireBus(OneWireBusBuildError),

    /// An open-drain two-wire bus topology is invalid.
    TwoWireBus(TwoWireBusBuildError),

    /// PROM image does not have the fixed hardware size.
    InvalidPromSize {
        /// Requested image size in bytes.
        size_bytes: usize,
    },

    /// CPU frequency cannot be represented by the IP32 timebase.
    InvalidProcessorFrequency {
        /// Requested processor frequency in hertz.
        frequency_hz: u64,
    },

    /// The R5000 rejected its processor or boot configuration.
    Cpu(R5000CpuError),

    /// A component identifier collided during registry construction.
    Registry(RegistryError),

    /// The completed topology could not resolve one required component.
    RegistryLookup(RegistryLookupError),
}

impl fmt::Display for Ip32MachineBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::JitUnavailable => write!(f, "JIT execution is unavailable in this build"),
            Self::JitInitialization(error) => {
                write!(f, "failed to initialize the JIT backend: {error}")
            }
            Self::Crime(error) => write!(f, "failed to construct CRIME: {error}"),
            Self::Mace(error) => write!(f, "failed to construct MACE: {error}"),
            Self::Ps2(error) => write!(f, "failed to construct PS/2 input: {error}"),
            Self::Rtc(error) => write!(f, "failed to construct DS1687: {error}"),
            Self::NicIdentity(error) => write!(f, "failed to construct DS2502: {error}"),
            Self::Uart(error) => write!(f, "failed to construct IP32 UART: {error}"),
            Self::IrqBus(error) => write!(f, "failed to construct IP32 IRQ bus: {error}"),
            Self::OneWireBus(error) => {
                write!(f, "failed to construct IP32 1-Wire bus: {error}")
            }
            Self::TwoWireBus(error) => {
                write!(f, "failed to construct IP32 two-wire bus: {error}")
            }
            Self::InvalidPromSize { size_bytes } => {
                write!(f, "invalid IP32 PROM size: {size_bytes} bytes")
            }
            Self::InvalidProcessorFrequency { frequency_hz } => {
                write!(f, "invalid IP32 processor frequency: {frequency_hz} Hz")
            }
            Self::Cpu(error) => write!(f, "failed to construct IP32 CPU: {error}"),
            Self::Registry(error) => write!(f, "failed to register IP32 component: {error}"),
            Self::RegistryLookup(error) => {
                write!(f, "failed to resolve IP32 component topology: {error}")
            }
        }
    }
}

impl std::error::Error for Ip32MachineBuildError {}

/// Error returned while dispatching an IP32 machine event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Ip32MachineDispatchError {
    /// A required component was missing or had an unexpected type.
    Registry(RegistryLookupError),

    /// The R5000 execution model failed.
    Cpu(R5000CpuError),

    /// CRIME reported an internal protocol error.
    Crime(CrimeError),

    /// MACE reported an internal protocol error.
    Mace(MaceError),

    /// The keyboard rejected a routed line transition.
    Keyboard(Ps2KeyboardError),

    /// The mouse rejected a routed line transition.
    Mouse(Ps2MouseError),

    /// A UART rejected host data.
    Uart(Uart16550Error),

    /// The IRQ bus rejected a source transaction.
    IrqBus(IrqBusRouteError),

    /// The 1-Wire bus rejected a source transition.
    OneWireBus(OneWireBusRouteError),

    /// An open-drain two-wire bus rejected a source transition.
    TwoWireBus(TwoWireBusRouteError),

    /// The R5000 rejected an IRQ bus delivery.
    CpuIrq(R5000IrqError),

    /// A follow-up event could not be scheduled.
    Scheduler(SchedulerError),

    /// Dispatch-local event-chain processing failed.
    EventChain(EventChainError),

    /// The reset generation counter was exhausted.
    GenerationOverflow,

    /// A completion named a controller not implemented by the current topology.
    UnexpectedController(ComponentId),
}

/// Error returned while scheduling deterministic host input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Ip32HostInputError {
    UnsupportedPort(se_device::bus::media::MediaPort),
    QueueFull(se_device::bus::media::MediaPort),
    Scheduler(SchedulerError),
}

impl fmt::Display for Ip32HostInputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPort(port) => {
                write!(
                    formatter,
                    "IP32 host byte injection is unsupported for {port:?}"
                )
            }
            Self::QueueFull(port) => write!(formatter, "IP32 host input queue is full: {port:?}"),
            Self::Scheduler(error) => {
                write!(formatter, "failed to schedule IP32 host input: {error}")
            }
        }
    }
}

impl std::error::Error for Ip32HostInputError {}

impl From<SchedulerError> for Ip32HostInputError {
    fn from(error: SchedulerError) -> Self {
        Self::Scheduler(error)
    }
}

impl fmt::Display for Ip32MachineDispatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registry(error) => write!(f, "IP32 component lookup failed: {error}"),
            Self::Cpu(error) => write!(f, "IP32 CPU execution failed: {error}"),
            Self::Crime(error) => write!(f, "CRIME dispatch failed: {error}"),
            Self::Mace(error) => write!(f, "MACE dispatch failed: {error}"),
            Self::Keyboard(error) => write!(f, "PS/2 keyboard dispatch failed: {error}"),
            Self::Mouse(error) => write!(f, "PS/2 mouse dispatch failed: {error}"),
            Self::Uart(error) => write!(f, "IP32 UART dispatch failed: {error}"),
            Self::IrqBus(error) => write!(f, "IP32 IRQ routing failed: {error}"),
            Self::OneWireBus(error) => write!(f, "IP32 1-Wire routing failed: {error}"),
            Self::TwoWireBus(error) => write!(f, "IP32 two-wire routing failed: {error}"),
            Self::CpuIrq(error) => write!(f, "IP32 CPU IRQ delivery failed: {error}"),
            Self::Scheduler(error) => write!(f, "IP32 event scheduling failed: {error}"),
            Self::EventChain(error) => write!(f, "IP32 event chain failed: {error}"),
            Self::GenerationOverflow => write!(f, "IP32 reset generation overflow"),
            Self::UnexpectedController(id) => {
                write!(f, "unsupported IP32 bus controller {id}")
            }
        }
    }
}

impl std::error::Error for Ip32MachineDispatchError {}

impl From<RegistryLookupError> for Ip32MachineDispatchError {
    fn from(error: RegistryLookupError) -> Self {
        Self::Registry(error)
    }
}

impl From<R5000CpuError> for Ip32MachineDispatchError {
    fn from(error: R5000CpuError) -> Self {
        Self::Cpu(error)
    }
}

impl From<CrimeError> for Ip32MachineDispatchError {
    fn from(error: CrimeError) -> Self {
        Self::Crime(error)
    }
}

impl From<MaceError> for Ip32MachineDispatchError {
    fn from(error: MaceError) -> Self {
        Self::Mace(error)
    }
}

impl From<Ps2KeyboardError> for Ip32MachineDispatchError {
    fn from(error: Ps2KeyboardError) -> Self {
        Self::Keyboard(error)
    }
}

impl From<Ps2MouseError> for Ip32MachineDispatchError {
    fn from(error: Ps2MouseError) -> Self {
        Self::Mouse(error)
    }
}

impl From<Uart16550Error> for Ip32MachineDispatchError {
    fn from(error: Uart16550Error) -> Self {
        Self::Uart(error)
    }
}

impl From<IrqBusRouteError> for Ip32MachineDispatchError {
    fn from(error: IrqBusRouteError) -> Self {
        Self::IrqBus(error)
    }
}

impl From<OneWireBusRouteError> for Ip32MachineDispatchError {
    fn from(error: OneWireBusRouteError) -> Self {
        Self::OneWireBus(error)
    }
}

impl From<TwoWireBusRouteError> for Ip32MachineDispatchError {
    fn from(error: TwoWireBusRouteError) -> Self {
        Self::TwoWireBus(error)
    }
}

impl From<R5000IrqError> for Ip32MachineDispatchError {
    fn from(error: R5000IrqError) -> Self {
        Self::CpuIrq(error)
    }
}

impl From<SchedulerError> for Ip32MachineDispatchError {
    fn from(error: SchedulerError) -> Self {
        Self::Scheduler(error)
    }
}

impl From<EventChainError> for Ip32MachineDispatchError {
    fn from(error: EventChainError) -> Self {
        Self::EventChain(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CpuClock {
    frequency_hz: u64,
    remainder: u64,
}

impl CpuClock {
    const fn new(frequency_hz: u64) -> Self {
        Self {
            frequency_hz,
            remainder: 0,
        }
    }

    fn reset(&mut self) {
        self.remainder = 0;
    }

    fn next_pclock_delay(&mut self) -> SimDuration {
        let base = IP32_TIMEBASE_HZ / self.frequency_hz;
        self.remainder += IP32_TIMEBASE_HZ % self.frequency_hz;
        let carry = self.remainder / self.frequency_hz;
        self.remainder %= self.frequency_hz;
        SimDuration::new(base + carry)
    }

    #[cfg(feature = "jit")]
    fn projection(self) -> FractionalClockProjection {
        FractionalClockProjection::new(IP32_TIMEBASE_HZ, self.frequency_hz, self.remainder)
    }

    #[cfg(feature = "jit")]
    fn advance_pclocks(&mut self, count: u64) -> SimDuration {
        let mut projection = self.projection();
        let elapsed = projection
            .advance(count)
            .expect("the bounded PClock advance cannot overflow");
        self.remainder = projection.remainder();
        elapsed
    }

    #[cfg(feature = "jit")]
    fn plan_boundary_budget(
        self,
        now: SimTime,
        deadline: SimTime,
        next_event: Option<SimTime>,
        maximum: usize,
    ) -> usize {
        if maximum == 0 {
            return 0;
        }
        let projection = self.projection();
        let deadline_ticks = deadline.get().saturating_sub(now.get());
        let maximum_ticks = projection
            .elapsed(maximum as u64)
            .expect("the bounded PClock projection cannot overflow")
            .get();
        if maximum_ticks <= deadline_ticks
            && next_event.is_none_or(|event| maximum_ticks <= event.get().saturating_sub(now.get()))
        {
            return maximum;
        }
        let deadline_boundary = deadline_ticks
            .checked_add(1)
            .and_then(|ticks| projection.cycles_until_elapsed_at_least(ticks))
            .unwrap_or(u64::MAX)
            .max(1);
        let event_boundary = next_event.map_or(u64::MAX, |event| {
            projection
                .cycles_until_elapsed_at_least(event.get().saturating_sub(now.get()))
                .unwrap_or(u64::MAX)
                .max(1)
        });
        usize::try_from(deadline_boundary.min(event_boundary))
            .unwrap_or(usize::MAX)
            .min(maximum)
    }
}

struct MachineControl {
    slots: HotComponentSlots,
    persistent_config: Ip32PersistentConfig,
    cpu_generation: u64,
    cpu_clock: CpuClock,
    host_generation: u64,
    host_capacities: MacePortConfig,
    host_reservations: [usize; 12],
    host_outputs: VecDeque<Ip32HostOutput>,
    host_output_units: [usize; 12],
    host_dropped_output_bytes: [u64; 12],
    latest_display_frame: Option<GbeFrame>,
    dropped_display_frames: u64,
    sysad_transactions: u64,
    memory_transactions: u64,
    cmi_transactions: u64,
    cgi_transactions: u64,
    #[cfg(feature = "jit")]
    fast_transaction_attempts: u64,
    #[cfg(feature = "jit")]
    fast_transaction_hits: u64,
    #[cfg(feature = "jit")]
    fast_transaction_fallbacks: u64,
    #[cfg(feature = "jit")]
    jit_code_fetches: Ip32StableCodeFetchStatistics,
    cpu_continuation_quantum: usize,
    inline_sysad_completion: bool,
    event_chain_policy: Ip32EventChainPolicy,
    #[cfg(feature = "jit")]
    jit_engine: Option<Mips4BlockEngine<CraneliftMips4Backend>>,
    #[cfg(feature = "jit")]
    jit_code_sources: Mips4CodeSourceCache,
    #[cfg(feature = "jit")]
    jit_ram_code_pages: Mips4RamCodePageSet,
    #[cfg(test)]
    capture_logical_transitions: bool,
    #[cfg(test)]
    logical_transitions: Vec<LogicalTransition>,
    #[cfg(test)]
    first_serial_output_time: Option<SimTime>,
    #[cfg(test)]
    first_cpu_idle_time: Option<SimTime>,
}

#[cfg(feature = "jit")]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Ip32StableCodeSource {
    SystemFlash,
    Sdram,
}

#[cfg(feature = "jit")]
impl Ip32StableCodeSource {
    const fn id(self) -> Mips4CodeSourceId {
        match self {
            Self::SystemFlash => Mips4CodeSourceId::new(1),
            Self::Sdram => Mips4CodeSourceId::new(2),
        }
    }

    fn from_guard(guard: Mips4CodeGuard) -> Result<Self, Ip32MachineDispatchError> {
        match guard.source_id {
            source_id if source_id == Self::SystemFlash.id() => Ok(Self::SystemFlash),
            source_id if source_id == Self::Sdram.id() => Ok(Self::Sdram),
            source_id => Err(Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
                format!(
                    "stable code guard used unknown IP32 source ID {}",
                    source_id.get()
                ),
            ))),
        }
    }
}

#[cfg(feature = "jit")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Ip32StableCodeFetchStatistics {
    system_flash: u64,
    sdram: u64,
}

#[cfg(feature = "jit")]
impl Ip32StableCodeFetchStatistics {
    fn record(&mut self, source: Ip32StableCodeSource, fetches: u64) {
        let counter = match source {
            Ip32StableCodeSource::SystemFlash => &mut self.system_flash,
            Ip32StableCodeSource::Sdram => &mut self.sdram,
        };
        *counter = counter.saturating_add(fetches);
    }
}

#[cfg(feature = "jit")]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct Mips4CodeSourceCacheKey {
    source: Ip32StableCodeSource,
    source_offset: u64,
    revision: u64,
    byte_count: u8,
    no_ecc: bool,
}

#[cfg(feature = "jit")]
#[derive(Clone, Debug, Eq, PartialEq)]
struct Mips4CodeSourceCacheEntry {
    bytes: Vec<u8>,
    fingerprint: u64,
}

#[cfg(feature = "jit")]
#[derive(Clone, Debug, Eq, PartialEq)]
struct Ip32CpuTransactionPlan {
    sysad: Ip32DirectSysAdPlan,
    delivery_time: SimTime,
    completion_time: SimTime,
    route: Ip32CpuTransactionRoute,
}

#[cfg(feature = "jit")]
#[derive(Clone, Debug, Eq, PartialEq)]
enum Ip32CpuTransactionRoute {
    InternalRegister,
    Memory {
        plan: CrimeDirectMemoryPlan,
        transaction: CrimeMemoryTransaction,
    },
}

#[cfg(feature = "jit")]
#[derive(Default)]
struct Mips4CodeSourceKeyHasher(u64);

#[cfg(feature = "jit")]
impl Mips4CodeSourceKeyHasher {
    fn mix(&mut self, value: u64) {
        self.0 ^= value.wrapping_add(0x9e37_79b9_7f4a_7c15);
        self.0 = self.0.rotate_left(27).wrapping_mul(0x94d0_49bb_1331_11eb);
    }
}

#[cfg(feature = "jit")]
impl Hasher for Mips4CodeSourceKeyHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for chunk in &mut chunks {
            self.mix(u64::from_ne_bytes(chunk.try_into().unwrap()));
        }
        let remainder = chunks.remainder();
        if !remainder.is_empty() {
            let mut tail = [0; 8];
            tail[..remainder.len()].copy_from_slice(remainder);
            self.mix(u64::from_ne_bytes(tail));
        }
    }

    fn write_u8(&mut self, value: u8) {
        self.mix(u64::from(value));
    }

    fn write_u64(&mut self, value: u64) {
        self.mix(value);
    }

    fn write_usize(&mut self, value: usize) {
        self.mix(value as u64);
    }
}

#[cfg(feature = "jit")]
type Mips4CodeSourceCache = HashMap<
    Mips4CodeSourceCacheKey,
    Mips4CodeSourceCacheEntry,
    BuildHasherDefault<Mips4CodeSourceKeyHasher>,
>;

#[cfg(feature = "jit")]
type Mips4RamCodePageSet = HashSet<u64, BuildHasherDefault<Mips4CodeSourceKeyHasher>>;

#[cfg(feature = "jit")]
const MIPS4_RAM_CODE_PAGE_SHIFT: u32 = 12;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HotComponentSlots {
    cpu: ComponentSlot<R5000Cpu>,
    sysad: ComponentSlot<Ip32SysAdBus>,
    crime: ComponentSlot<Crime>,
    memory: ComponentSlot<CrimeMemoryBus>,
    sdram: ComponentSlot<CrimeSdram>,
    cmi: ComponentSlot<CrimeCmiBus>,
    cgi: ComponentSlot<CrimeCgiBus>,
    gbe: ComponentSlot<Gbe>,
    mace: ComponentSlot<Mace>,
    isa: ComponentSlot<IsaBus>,
    prom: ComponentSlot<SystemFlash>,
    rtc: ComponentSlot<Ds1687>,
    serial: [ComponentSlot<Uart16550>; 2],
    parallel: ComponentSlot<Ieee1284>,
    one_wire: ComponentSlot<OneWireBus>,
    gbe_ddc: [ComponentSlot<TwoWireBus>; 2],
    ps2_buses: [ComponentSlot<TwoWireBus>; 2],
    keyboard: ComponentSlot<Ps2Keyboard>,
    mouse: ComponentSlot<Ps2Mouse>,
    nic_identity: ComponentSlot<Ds2502>,
}

impl HotComponentSlots {
    fn resolve(registry: &ComponentRegistry) -> Result<Self, RegistryLookupError> {
        Ok(Self {
            cpu: registry.resolve(component_ids::CPU0)?,
            sysad: registry.resolve(component_ids::CPU_SYSAD_BUS)?,
            crime: registry.resolve(component_ids::CRIME)?,
            memory: registry.resolve(component_ids::CRIME_MEMORY_DOMAIN)?,
            sdram: registry.resolve(component_ids::RAM)?,
            cmi: registry.resolve(component_ids::CRIME_MACE_LINK)?,
            cgi: registry.resolve(component_ids::CRIME_GBE_LINK)?,
            gbe: registry.resolve(component_ids::GBE)?,
            mace: registry.resolve(component_ids::MACE)?,
            isa: registry.resolve(component_ids::ISA_BUS)?,
            prom: registry.resolve(component_ids::PROM)?,
            rtc: registry.resolve(component_ids::RTC)?,
            serial: [
                registry.resolve(component_ids::SERIAL0)?,
                registry.resolve(component_ids::SERIAL1)?,
            ],
            parallel: registry.resolve(component_ids::PARALLEL_PORT)?,
            one_wire: registry.resolve(component_ids::ONE_WIRE_BUS)?,
            gbe_ddc: [
                registry.resolve(component_ids::GBE_CRT_DDC_BUS)?,
                registry.resolve(component_ids::GBE_FLAT_PANEL_DDC_BUS)?,
            ],
            ps2_buses: [
                registry.resolve(component_ids::KEYBOARD_PS2_BUS)?,
                registry.resolve(component_ids::MOUSE_PS2_BUS)?,
            ],
            keyboard: registry.resolve(component_ids::KEYBOARD)?,
            mouse: registry.resolve(component_ids::MOUSE)?,
            nic_identity: registry.resolve(component_ids::NIC_IDENTITY)?,
        })
    }
}

/// Cumulative performance counters for one IP32 machine.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Ip32PerformanceSnapshot {
    /// Current internal simulated time.
    pub sim_time: SimTime,

    /// Runtime scheduler counters.
    pub runtime: RuntimeStatistics,

    /// R5000 execution counters.
    pub cpu: R5000CpuStatistics,

    /// CPU transactions routed onto SysAD.
    pub sysad_transactions: u64,

    /// Transactions routed onto the CRIME memory domain.
    pub memory_transactions: u64,

    /// Transactions routed onto CMI.
    pub cmi_transactions: u64,

    /// Transactions routed onto CGI.
    pub cgi_transactions: u64,

    /// Derived JIT execution counters.
    pub jit: Ip32JitPerformanceSnapshot,
}

/// Derived basic-block execution counters for one IP32 machine.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Ip32JitPerformanceSnapshot {
    /// Guest operations entered by the IR interpreter.
    pub interpreted_operations: u64,

    /// Guest operations entered by native code.
    pub native_operations: u64,

    /// Native Region function entries.
    pub region_entries: u64,

    /// Guest operations entered by native Regions.
    pub region_operations: u64,

    /// Regions compiled since the latest engine reset.
    pub region_compilations: u64,

    /// Region exits through an uncompiled successor edge.
    pub region_cold_side_exits: u64,

    /// Region exits at a retirement-budget boundary.
    pub region_budget_side_exits: u64,

    /// Region exits through a typed runtime operation.
    pub region_runtime_side_exits: u64,

    /// Region entries rejected by a derived-state guard.
    pub region_guard_side_exits: u64,

    /// Typed runtime helper calls.
    pub runtime_calls: u64,

    /// Instructions translated after real dynamic fetches.
    pub dynamic_fetches: u64,

    /// Instructions fetched through stable code windows.
    pub fast_fetches: u64,

    /// Stable instructions fetched from System Flash.
    pub system_flash_fetches: u64,

    /// Stable instructions fetched from SDRAM.
    pub sdram_fetches: u64,

    /// Native blocks compiled since the latest engine reset.
    pub compiled_blocks: u64,

    /// Whole derived-cache resets.
    pub cache_resets: u64,

    /// CPU transactions considered for synchronous direct completion.
    pub fast_transaction_attempts: u64,

    /// CPU transactions completed through a synchronous direct plan.
    pub fast_transaction_hits: u64,

    /// CPU transactions retained on the exact scheduled path.
    pub fast_transaction_fallbacks: u64,
}

/// SGI O2 IP32 machine with runtime-owned hardware components.
pub struct Ip32Machine<S = NoopTraceSink> {
    runtime: Runtime<Ip32Event, S>,
    control: MachineControl,
}

impl Ip32Machine<NoopTraceSink> {
    /// Creates the default IP32 machine with a noop trace sink.
    pub fn new() -> Self {
        Self::from_config(Ip32MachineConfig::default())
            .expect("the default IP32 machine configuration must be valid")
    }

    /// Creates a configured IP32 machine with a noop trace sink.
    pub fn from_config(config: Ip32MachineConfig) -> Result<Self, Ip32MachineBuildError> {
        Self::from_config_with_trace_sink(config, NoopTraceSink)
    }
}

impl Default for Ip32Machine<NoopTraceSink> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Ip32Machine<S> {
    /// Creates the default IP32 machine with the given trace sink.
    pub fn with_trace_sink(sink: S) -> Self {
        Self::from_config_with_trace_sink(Ip32MachineConfig::default(), sink)
            .expect("the default IP32 machine configuration must be valid")
    }

    /// Creates a configured IP32 machine with the given trace sink.
    pub fn from_config_with_trace_sink(
        config: Ip32MachineConfig,
        sink: S,
    ) -> Result<Self, Ip32MachineBuildError> {
        validate_config(&config)?;
        #[cfg(feature = "jit")]
        let jit_engine = if config.jit_enabled {
            Some(Mips4BlockEngine::new(
                CraneliftMips4Backend::new()
                    .map_err(|error| Ip32MachineBuildError::JitInitialization(error.to_string()))?,
            ))
        } else {
            None
        };
        #[cfg(feature = "jit")]
        let event_chain_policy = if jit_engine.is_some() {
            Ip32EventChainPolicy::disabled()
        } else {
            Ip32EventChainPolicy::all()
        };
        #[cfg(not(feature = "jit"))]
        let event_chain_policy = Ip32EventChainPolicy::all();
        let persistent_config = Ip32PersistentConfig::from_machine_config(&config);
        let processor_frequency_hz = config.processor.processor_frequency_hz;
        let cpu = R5000Cpu::new(
            component_ids::CPU0,
            "R5000 CPU 0",
            config.processor,
            config.boot_mode,
        )
        .map_err(Ip32MachineBuildError::Cpu)?;
        let crime = Crime::new(
            component_ids::CRIME,
            "CRIME 1.1",
            config.crime,
            IP32_TIMEBASE_HZ,
            component_ids::RAM,
            component_ids::MACE,
            component_ids::GBE,
        )
        .map_err(Ip32MachineBuildError::Crime)?;
        let irq_bus = IrqBus::new(
            component_ids::CPU_IRQ_BUS,
            "CPU IRQ bus",
            [IrqRoute {
                source: IrqSource {
                    component: component_ids::CRIME,
                    output: CRIME_IRQ_OUTPUT,
                },
                target: IrqTarget {
                    component: component_ids::CPU0,
                    input: R5000_IRQ_IP2,
                },
            }],
        )
        .map_err(Ip32MachineBuildError::IrqBus)?;
        let mace_irq_bus = IrqBus::new(
            component_ids::MACE_IRQ_BUS,
            "MACE IRQ bus",
            [
                IrqRoute {
                    source: IrqSource {
                        component: component_ids::RTC,
                        output: DS1687_IRQ_OUTPUT,
                    },
                    target: IrqTarget {
                        component: component_ids::MACE,
                        input: MACE_IRQ_RTC,
                    },
                },
                IrqRoute {
                    source: IrqSource {
                        component: component_ids::SERIAL0,
                        output: UART16550_IRQ_OUTPUT,
                    },
                    target: IrqTarget {
                        component: component_ids::MACE,
                        input: MACE_IRQ_SERIAL0,
                    },
                },
                IrqRoute {
                    source: IrqSource {
                        component: component_ids::SERIAL1,
                        output: UART16550_IRQ_OUTPUT,
                    },
                    target: IrqTarget {
                        component: component_ids::MACE,
                        input: MACE_IRQ_SERIAL1,
                    },
                },
                IrqRoute {
                    source: IrqSource {
                        component: component_ids::PARALLEL_PORT,
                        output: IEEE1284_IRQ_OUTPUT,
                    },
                    target: IrqTarget {
                        component: component_ids::MACE,
                        input: MACE_IRQ_PARALLEL,
                    },
                },
            ],
        )
        .map_err(Ip32MachineBuildError::IrqBus)?;
        let one_wire_bus = OneWireBus::new(
            component_ids::ONE_WIRE_BUS,
            "MACE board-identity 1-Wire bus",
            [component_ids::MACE, component_ids::NIC_IDENTITY],
        )
        .map_err(Ip32MachineBuildError::OneWireBus)?;
        let gbe_crt_ddc = TwoWireBus::new(
            component_ids::GBE_CRT_DDC_BUS,
            "GBE CRT DDC bus",
            [component_ids::GBE],
        )
        .map_err(Ip32MachineBuildError::TwoWireBus)?;
        let gbe_flat_panel_ddc = TwoWireBus::new(
            component_ids::GBE_FLAT_PANEL_DDC_BUS,
            "GBE flat-panel DDC bus",
            [component_ids::GBE],
        )
        .map_err(Ip32MachineBuildError::TwoWireBus)?;
        let keyboard_ps2_bus = TwoWireBus::new(
            component_ids::KEYBOARD_PS2_BUS,
            "MACE keyboard PS/2 bus",
            [component_ids::MACE, component_ids::KEYBOARD],
        )
        .map_err(Ip32MachineBuildError::TwoWireBus)?;
        let mouse_ps2_bus = TwoWireBus::new(
            component_ids::MOUSE_PS2_BUS,
            "MACE mouse PS/2 bus",
            [component_ids::MACE, component_ids::MOUSE],
        )
        .map_err(Ip32MachineBuildError::TwoWireBus)?;
        let keyboard = Ps2Keyboard::new(
            component_ids::KEYBOARD,
            "IBM enhanced PS/2 keyboard",
            Ps2Wiring {
                controller: component_ids::MACE,
                bus: component_ids::KEYBOARD_PS2_BUS,
            },
            IP32_TIMEBASE_HZ,
        )
        .map_err(Ip32MachineBuildError::Ps2)?;
        let mouse = Ps2Mouse::new(
            component_ids::MOUSE,
            "Standard three-button PS/2 mouse",
            Ps2Wiring {
                controller: component_ids::MACE,
                bus: component_ids::MOUSE_PS2_BUS,
            },
            IP32_TIMEBASE_HZ,
        )
        .map_err(Ip32MachineBuildError::Ps2)?;
        let mace = Mace::new(
            component_ids::MACE,
            "MACE 2.0",
            config.mace,
            MaceWiring {
                crime: component_ids::CRIME,
                pci_bus: component_ids::PCI_BUS,
                pci_devices: [
                    component_ids::SCSI_CONTROLLER,
                    component_ids::PCI_SLOT0,
                    component_ids::PCI_SLOT0,
                    component_ids::PCI_SLOT0,
                    component_ids::PCI_SLOT0,
                ],
                pci_absent: component_ids::PCI_SLOT0,
                isa_bus: component_ids::ISA_BUS,
                prom: component_ids::PROM,
                rtc: component_ids::RTC,
                serial: [component_ids::SERIAL0, component_ids::SERIAL1],
                parallel: component_ids::PARALLEL_PORT,
                ps2_buses: [
                    component_ids::KEYBOARD_PS2_BUS,
                    component_ids::MOUSE_PS2_BUS,
                ],
                external_links: MaceExternalLinks {
                    i2c: [component_ids::VIDEO_INPUT0, component_ids::VIDEO_INPUT1],
                    audio: component_ids::AUDIO_SUBSYSTEM,
                    video_input_ab: component_ids::VIDEO_INPUT0,
                    video_input_cd: component_ids::VIDEO_INPUT1,
                    video_output: component_ids::VIDEO_OUTPUT,
                    ethernet: component_ids::ETHERNET_CONTROLLER,
                },
            },
            IP32_TIMEBASE_HZ,
        )
        .map_err(Ip32MachineBuildError::Mace)?;
        let rtc = Ds1687::new(
            component_ids::RTC,
            "DS1687 RTC/NVRAM",
            IP32_TIMEBASE_HZ,
            config.rtc,
        )
        .map_err(Ip32MachineBuildError::Rtc)?;
        let nic_identity = Ds2502::new(
            component_ids::NIC_IDENTITY,
            "DS2502 board identity",
            component_ids::MACE,
            IP32_TIMEBASE_HZ,
            config.nic_identity,
        )
        .map_err(Ip32MachineBuildError::NicIdentity)?;
        let uart_config = Uart16550Config {
            input_clock_hz: UART_INPUT_CLOCK_HZ,
            timebase_hz: IP32_TIMEBASE_HZ,
            external_queue_capacity: config.mace.ports.byte_stream_bytes,
        };
        let serial0 = Uart16550::new(component_ids::SERIAL0, "Serial port 0", uart_config)
            .map_err(Ip32MachineBuildError::Uart)?;
        let serial1 = Uart16550::new(component_ids::SERIAL1, "Serial port 1", uart_config)
            .map_err(Ip32MachineBuildError::Uart)?;

        let mut runtime = Runtime::with_trace_sink(sink);
        let registry = runtime.registry_mut();
        insert_component(registry, Box::new(cpu))?;
        insert_component(
            registry,
            Box::new(Ip32SysAdBus::new(
                component_ids::CPU_SYSAD_BUS,
                "CPU SysAD bus",
                component_ids::CPU0,
                component_ids::CRIME,
                IP32_TIMEBASE_HZ,
                66_666_500,
            )),
        )?;
        insert_component(registry, Box::new(irq_bus))?;
        insert_component(registry, Box::new(mace_irq_bus))?;
        insert_component(registry, Box::new(one_wire_bus))?;
        insert_component(registry, Box::new(gbe_crt_ddc))?;
        insert_component(registry, Box::new(gbe_flat_panel_ddc))?;
        insert_component(registry, Box::new(keyboard_ps2_bus))?;
        insert_component(registry, Box::new(mouse_ps2_bus))?;
        insert_component(registry, Box::new(crime))?;
        insert_component(
            registry,
            Box::new(CrimeMemoryBus::new(
                component_ids::CRIME_MEMORY_DOMAIN,
                "CRIME memory domain",
                component_ids::RAM,
                IP32_TIMEBASE_HZ,
                SimDuration::new(SDRAM_REFRESH_TICKS),
            )),
        )?;
        insert_component(
            registry,
            Box::new(CrimeCmiBus::new(
                component_ids::CRIME_MACE_LINK,
                "CRIME CMI link",
                IP32_TIMEBASE_HZ,
            )),
        )?;
        insert_component(
            registry,
            Box::new(CrimeCgiBus::new(
                component_ids::CRIME_GBE_LINK,
                "CRIME CGI link",
                IP32_TIMEBASE_HZ,
            )),
        )?;
        insert_component(
            registry,
            Box::new(CrimeSdram::new(
                component_ids::RAM,
                "CRIME SDRAM",
                config.crime.memory,
            )),
        )?;
        insert_component(
            registry,
            Box::new(IsaBus::new(
                component_ids::ISA_BUS,
                "MACE ISA bus",
                SimDuration::new(ISA_CYCLE_TICKS),
            )),
        )?;
        insert_component(
            registry,
            Box::new(PciBus::new(
                component_ids::PCI_BUS,
                "MACE PCI bus",
                SimDuration::new(PCI_CYCLE_TICKS),
            )),
        )?;
        insert_component(
            registry,
            Box::new(I2cBus::new(component_ids::I2C_BUS0, "MACE I2C bus 0")),
        )?;
        insert_component(
            registry,
            Box::new(I2cBus::new(component_ids::I2C_BUS1, "MACE I2C bus 1")),
        )?;
        insert_component(
            registry,
            Box::new(MediaBus::new(
                component_ids::MACE_MEDIA_BUS,
                "MACE media bus",
            )),
        )?;
        insert_component(registry, Box::new(mace))?;
        insert_component(registry, Box::new(keyboard))?;
        insert_component(registry, Box::new(mouse))?;
        insert_component(
            registry,
            Box::new(Gbe::new(
                component_ids::GBE,
                "Graphics Back End",
                IP32_TIMEBASE_HZ,
                GbeWiring {
                    crime: component_ids::CRIME,
                    crt_ddc: component_ids::GBE_CRT_DDC_BUS,
                    flat_panel_ddc: component_ids::GBE_FLAT_PANEL_DDC_BUS,
                },
            )),
        )?;
        insert_component(
            registry,
            Box::new(Ip32StubEndpoint::new(component_ids::VICE, "VICE endpoint")),
        )?;
        insert_component(
            registry,
            Box::new(SystemFlash::new(
                component_ids::PROM,
                "System flash",
                config.prom_image,
            )),
        )?;
        insert_component(registry, Box::new(rtc))?;
        insert_component(registry, Box::new(nic_identity))?;
        insert_component(registry, Box::new(serial0))?;
        insert_component(registry, Box::new(serial1))?;
        insert_component(
            registry,
            Box::new(PciConfigurationEndpoint::new(
                component_ids::SCSI_CONTROLLER,
                "PCI SCSI configuration endpoint",
                0x9004,
                0x8078,
                0x010000,
                0,
            )),
        )?;
        insert_component(
            registry,
            Box::new(Ieee1284::new(
                component_ids::PARALLEL_PORT,
                "IEEE 1284 parallel port",
            )),
        )?;

        let slots = HotComponentSlots::resolve(runtime.registry())
            .map_err(Ip32MachineBuildError::RegistryLookup)?;
        Ok(Self {
            runtime,
            control: MachineControl {
                slots,
                persistent_config,
                cpu_generation: 0,
                cpu_clock: CpuClock::new(processor_frequency_hz),
                host_generation: 0,
                host_capacities: config.mace.ports,
                host_reservations: [0; 12],
                host_outputs: VecDeque::new(),
                host_output_units: [0; 12],
                host_dropped_output_bytes: [0; 12],
                latest_display_frame: None,
                dropped_display_frames: 0,
                sysad_transactions: 0,
                memory_transactions: 0,
                cmi_transactions: 0,
                cgi_transactions: 0,
                #[cfg(feature = "jit")]
                fast_transaction_attempts: 0,
                #[cfg(feature = "jit")]
                fast_transaction_hits: 0,
                #[cfg(feature = "jit")]
                fast_transaction_fallbacks: 0,
                #[cfg(feature = "jit")]
                jit_code_fetches: Ip32StableCodeFetchStatistics::default(),
                cpu_continuation_quantum: DEFAULT_CPU_CONTINUATION_QUANTUM,
                inline_sysad_completion: true,
                event_chain_policy,
                #[cfg(feature = "jit")]
                jit_engine,
                #[cfg(feature = "jit")]
                jit_code_sources: Mips4CodeSourceCache::default(),
                #[cfg(feature = "jit")]
                jit_ram_code_pages: Mips4RamCodePageSet::default(),
                #[cfg(test)]
                capture_logical_transitions: false,
                #[cfg(test)]
                logical_transitions: Vec::new(),
                #[cfg(test)]
                first_serial_output_time: None,
                #[cfg(test)]
                first_cpu_idle_time: None,
            },
        })
    }

    /// Returns an immutable runtime reference.
    pub const fn runtime(&self) -> &Runtime<Ip32Event, S> {
        &self.runtime
    }

    /// Returns a mutable runtime reference.
    pub const fn runtime_mut(&mut self) -> &mut Runtime<Ip32Event, S> {
        &mut self.runtime
    }

    /// Consumes the machine and returns the owned runtime.
    pub fn into_runtime(self) -> Runtime<Ip32Event, S> {
        self.runtime
    }

    /// Schedules one host-neutral input at an explicit simulation time.
    pub fn schedule_host_input(
        &mut self,
        at: SimTime,
        input: Ip32HostInput,
    ) -> Result<ScheduledEventId, Ip32HostInputError>
    where
        S: TraceSink,
    {
        if matches!(input.port, MediaPort::Keyboard | MediaPort::Mouse) {
            return Err(Ip32HostInputError::UnsupportedPort(input.port));
        }
        let index = media_port_index(input.port);
        let units = host_payload_units(&input.payload);
        let queued = match input.port {
            MediaPort::Serial0 | MediaPort::Serial1 => self
                .runtime
                .registry()
                .get_typed::<Uart16550>(serial_component(input.port))
                .expect("the IP32 UART component must remain registered")
                .external_receive_len(),
            _ => self
                .runtime
                .registry()
                .get_typed::<Mace>(component_ids::MACE)
                .expect("the IP32 MACE component must remain registered")
                .host_input_len(input.port),
        };
        if queued + self.control.host_reservations[index] + units
            > media_port_capacity(self.control.host_capacities, input.port)
        {
            return Err(Ip32HostInputError::QueueFull(input.port));
        }
        let id = self.runtime.schedule_at(
            at,
            component_ids::MACE_MEDIA_BUS,
            Ip32Event::HostInput {
                generation: self.control.host_generation,
                input,
            },
        )?;
        self.control.host_reservations[index] += units;
        Ok(id)
    }

    /// Schedules one physical keyboard or mouse transition.
    pub fn schedule_input(
        &mut self,
        at: SimTime,
        input: Ip32InputEvent,
    ) -> Result<ScheduledEventId, SchedulerError>
    where
        S: TraceSink,
    {
        self.runtime.schedule_at(
            at,
            component_ids::MACHINE,
            Ip32Event::Input {
                generation: self.control.host_generation,
                input,
            },
        )
    }

    /// Schedules bytes arriving at one physical serial connector.
    pub fn schedule_serial_input(
        &mut self,
        at: SimTime,
        port: Ip32SerialPort,
        bytes: Vec<u8>,
    ) -> Result<ScheduledEventId, Ip32HostInputError>
    where
        S: TraceSink,
    {
        self.schedule_host_input(
            at,
            Ip32HostInput {
                port: match port {
                    Ip32SerialPort::Serial1 => MediaPort::Serial0,
                    Ip32SerialPort::Serial2 => MediaPort::Serial1,
                },
                payload: MediaPayload::Bytes(bytes),
            },
        )
    }

    /// Schedules one deterministic external GBE pin or clock input.
    pub fn schedule_gbe_external_input(
        &mut self,
        at: SimTime,
        input: GbeExternalInput,
    ) -> Result<ScheduledEventId, SchedulerError>
    where
        S: TraceSink,
    {
        self.runtime
            .schedule_at(at, component_ids::GBE, Ip32Event::GbeInput(input))
    }

    /// Returns current CRT, flat-panel, field, auxiliary, and GPIO pin levels.
    pub fn gbe_output_pins(&mut self) -> GbeOutputPins {
        let now = self.runtime.now();
        let gbe = self
            .runtime
            .registry_mut()
            .get_typed_mut::<Gbe>(component_ids::GBE)
            .expect("the IP32 GBE component must remain registered");
        gbe.observe_time(now);
        gbe.output_pins()
    }

    /// Removes the oldest host-neutral output produced by the machine.
    pub fn poll_host_output(&mut self) -> Option<Ip32HostOutput> {
        let output = self.control.host_outputs.pop_front()?;
        let index = media_port_index(output.port);
        self.control.host_output_units[index] = self.control.host_output_units[index]
            .saturating_sub(host_payload_units(&output.payload));
        Some(output)
    }

    /// Removes the oldest serial output while preserving other host outputs.
    pub fn poll_serial_output(&mut self) -> Option<Ip32SerialOutput> {
        let position = self.control.host_outputs.iter().position(|output| {
            matches!(output.port, MediaPort::Serial0 | MediaPort::Serial1)
                && matches!(output.payload, MediaPayload::Bytes(_))
        })?;
        let output = self
            .control
            .host_outputs
            .remove(position)
            .expect("located serial output must remain queued");
        let MediaPayload::Bytes(bytes) = output.payload else {
            unreachable!("serial output was validated as a byte payload")
        };
        let index = media_port_index(output.port);
        self.control.host_output_units[index] =
            self.control.host_output_units[index].saturating_sub(bytes.len());
        Some(Ip32SerialOutput {
            port: if output.port == MediaPort::Serial0 {
                Ip32SerialPort::Serial1
            } else {
                Ip32SerialPort::Serial2
            },
            bytes,
        })
    }

    /// Returns host-bound data-loss counters.
    pub const fn host_io_stats(&self) -> Ip32HostIoStats {
        Ip32HostIoStats {
            dropped_output_bytes: self.control.host_dropped_output_bytes,
        }
    }

    /// Returns the newest completed display frame without consuming it.
    pub const fn latest_display_frame(&self) -> Option<&GbeFrame> {
        self.control.latest_display_frame.as_ref()
    }

    /// Removes and returns the newest completed display frame.
    pub fn take_display_frame(&mut self) -> Option<GbeFrame> {
        self.control.latest_display_frame.take()
    }

    /// Returns the number of completed frames overwritten before consumption.
    pub const fn dropped_display_frame_count(&self) -> u64 {
        self.control.dropped_display_frames
    }

    /// Returns a cumulative performance snapshot.
    pub fn performance_snapshot(&self) -> Ip32PerformanceSnapshot {
        #[cfg(feature = "jit")]
        let jit = self
            .control
            .jit_engine
            .as_ref()
            .map(|engine| {
                let statistics = engine.statistics();
                Ip32JitPerformanceSnapshot {
                    interpreted_operations: statistics.interpreted_operations,
                    native_operations: statistics.native_operations,
                    region_entries: statistics.region_entries,
                    region_operations: statistics.region_operations,
                    region_compilations: statistics.region_compilations,
                    region_cold_side_exits: statistics.region_cold_side_exits,
                    region_budget_side_exits: statistics.region_budget_side_exits,
                    region_runtime_side_exits: statistics.region_runtime_side_exits,
                    region_guard_side_exits: statistics.region_guard_side_exits,
                    runtime_calls: statistics.runtime_calls,
                    dynamic_fetches: statistics.dynamic_fetches,
                    fast_fetches: statistics.fast_fetches,
                    system_flash_fetches: self.control.jit_code_fetches.system_flash,
                    sdram_fetches: self.control.jit_code_fetches.sdram,
                    compiled_blocks: statistics.compiled_blocks,
                    cache_resets: statistics.cache_resets,
                    fast_transaction_attempts: self.control.fast_transaction_attempts,
                    fast_transaction_hits: self.control.fast_transaction_hits,
                    fast_transaction_fallbacks: self.control.fast_transaction_fallbacks,
                }
            })
            .unwrap_or_default();
        #[cfg(not(feature = "jit"))]
        let jit = Ip32JitPerformanceSnapshot::default();
        Ip32PerformanceSnapshot {
            sim_time: self.runtime.now(),
            runtime: self.runtime.statistics(),
            cpu: self
                .runtime
                .registry()
                .get_typed::<R5000Cpu>(component_ids::CPU0)
                .expect("the IP32 CPU component must remain registered")
                .statistics(),
            sysad_transactions: self.control.sysad_transactions,
            memory_transactions: self.control.memory_transactions,
            cmi_transactions: self.control.cmi_transactions,
            cgi_transactions: self.control.cgi_transactions,
            jit,
        }
    }

    /// Captures the physical DS1687 RTC/NVRAM domain at the current simulated time.
    ///
    /// This state does not contain the IP32 PROM environment stored in System
    /// Flash.
    pub fn rtc_persistent_state(&self) -> Result<Ds1687PersistentState, RegistryLookupError> {
        self.runtime
            .registry()
            .get_typed::<Ds1687>(component_ids::RTC)
            .map(|rtc| rtc.persistent_state(self.runtime.now()))
    }

    /// Applies battery-backed RTC and NVRAM data to a newly constructed machine.
    pub fn restore_rtc_persistent_state(
        &mut self,
        state: &Ds1687PersistentState,
    ) -> Result<(), RestoreRtcPersistentStateError> {
        let now = self.runtime.now();
        self.runtime
            .registry_mut()
            .get_typed_mut::<Ds1687>(component_ids::RTC)
            .map_err(RestoreRtcPersistentStateError::Registry)?
            .restore_persistent_state(state, now)
            .map_err(RestoreRtcPersistentStateError::Rtc)
    }

    /// Captures guest-programmed System Flash bytes relative to the base PROM image.
    ///
    /// On IP32, this includes the PROM environment and is independent of the
    /// DS1687 RTC/NVRAM state.
    pub fn system_flash_persistent_state(
        &self,
    ) -> Result<SystemFlashPersistentState, RegistryLookupError> {
        self.runtime
            .registry()
            .get_typed::<SystemFlash>(component_ids::PROM)
            .map(SystemFlash::persistent_state)
    }

    /// Applies guest-programmed System Flash bytes to a newly constructed machine.
    pub fn restore_system_flash_persistent_state(
        &mut self,
        state: &SystemFlashPersistentState,
    ) -> Result<(), RestoreSystemFlashPersistentStateError> {
        self.runtime
            .registry_mut()
            .get_typed_mut::<SystemFlash>(component_ids::PROM)
            .map_err(RestoreSystemFlashPersistentStateError::Registry)?
            .restore_persistent_state(state)
            .map_err(RestoreSystemFlashPersistentStateError::SystemFlash)
    }

    /// Captures the complete deterministic machine state at an outer event boundary.
    pub fn save_state(&self) -> Result<Ip32MachineState, Ip32StateError>
    where
        Ip32Event: Clone,
    {
        let registry = self.runtime.registry();
        let component = |id| {
            registry
                .get(id)
                .ok_or(RegistryLookupError::MissingComponent { id })
        };
        let _ = component(component_ids::CPU0).map_err(Ip32StateError::Registry)?;
        Ok(Ip32MachineState {
            schema_version: IP32_STATE_SCHEMA_VERSION,
            config: self.control.persistent_config.clone(),
            runtime: self.runtime.save_state(),
            control: MachineControlState {
                cpu_generation: self.control.cpu_generation,
                cpu_clock_remainder: self.control.cpu_clock.remainder,
                host_generation: self.control.host_generation,
                host_capacities: self.control.host_capacities,
                host_reservations: self.control.host_reservations,
                host_outputs: self.control.host_outputs.iter().cloned().collect(),
                host_output_units: self.control.host_output_units,
                host_dropped_output_bytes: self.control.host_dropped_output_bytes,
                latest_display_frame: self.control.latest_display_frame.clone(),
                dropped_display_frames: self.control.dropped_display_frames,
                sysad_transactions: self.control.sysad_transactions,
                memory_transactions: self.control.memory_transactions,
                cmi_transactions: self.control.cmi_transactions,
                cgi_transactions: self.control.cgi_transactions,
                cpu_continuation_quantum: self.control.cpu_continuation_quantum,
                inline_sysad_completion: self.control.inline_sysad_completion,
                fusion_sysad: self.control.event_chain_policy.sysad,
                fusion_memory: self.control.event_chain_policy.memory,
                fusion_cmi: self.control.event_chain_policy.cmi,
                fusion_cgi: self.control.event_chain_policy.cgi,
                fusion_isa: self.control.event_chain_policy.isa,
                fusion_budget: self.control.event_chain_policy.budget,
            },
            cpu: save_component(registry, component_ids::CPU0, R5000Cpu::save_state)?,
            sysad: save_component(
                registry,
                component_ids::CPU_SYSAD_BUS,
                Ip32SysAdBus::save_state,
            )?,
            cpu_irq: save_component(registry, component_ids::CPU_IRQ_BUS, IrqBus::save_state)?,
            mace_irq: save_component(registry, component_ids::MACE_IRQ_BUS, IrqBus::save_state)?,
            one_wire: save_component(
                registry,
                component_ids::ONE_WIRE_BUS,
                OneWireBus::save_state,
            )?,
            gbe_ddc: [
                save_component(
                    registry,
                    component_ids::GBE_CRT_DDC_BUS,
                    TwoWireBus::save_state,
                )?,
                save_component(
                    registry,
                    component_ids::GBE_FLAT_PANEL_DDC_BUS,
                    TwoWireBus::save_state,
                )?,
            ],
            ps2_buses: [
                save_component(
                    registry,
                    component_ids::KEYBOARD_PS2_BUS,
                    TwoWireBus::save_state,
                )?,
                save_component(
                    registry,
                    component_ids::MOUSE_PS2_BUS,
                    TwoWireBus::save_state,
                )?,
            ],
            crime: save_component(registry, component_ids::CRIME, Crime::save_state)?,
            memory_bus: save_component(
                registry,
                component_ids::CRIME_MEMORY_DOMAIN,
                CrimeMemoryBus::save_state,
            )?,
            cmi: save_component(
                registry,
                component_ids::CRIME_MACE_LINK,
                CrimeCmiBus::save_state,
            )?,
            cgi: save_component(
                registry,
                component_ids::CRIME_GBE_LINK,
                CrimeCgiBus::save_state,
            )?,
            sdram: save_component(registry, component_ids::RAM, CrimeSdram::save_state)?,
            isa: save_component(registry, component_ids::ISA_BUS, IsaBus::save_state)?,
            pci: save_component(registry, component_ids::PCI_BUS, PciBus::save_state)?,
            i2c: [
                save_component(registry, component_ids::I2C_BUS0, I2cBus::save_state)?,
                save_component(registry, component_ids::I2C_BUS1, I2cBus::save_state)?,
            ],
            media: save_component(
                registry,
                component_ids::MACE_MEDIA_BUS,
                MediaBus::save_state,
            )?,
            mace: save_component(registry, component_ids::MACE, Mace::save_state)?,
            keyboard: save_component(registry, component_ids::KEYBOARD, Ps2Keyboard::save_state)?,
            mouse: save_component(registry, component_ids::MOUSE, Ps2Mouse::save_state)?,
            gbe: save_component(registry, component_ids::GBE, Gbe::save_state)?,
            vice: save_component(registry, component_ids::VICE, Ip32StubEndpoint::save_state)?,
            system_flash: registry
                .get_typed::<SystemFlash>(component_ids::PROM)
                .map_err(Ip32StateError::Registry)?
                .save_state(),
            rtc: registry
                .get_typed::<Ds1687>(component_ids::RTC)
                .map_err(Ip32StateError::Registry)?
                .save_state(),
            nic_identity: save_component(
                registry,
                component_ids::NIC_IDENTITY,
                Ds2502::save_state,
            )?,
            serial: [
                save_component(registry, component_ids::SERIAL0, Uart16550::save_state)?,
                save_component(registry, component_ids::SERIAL1, Uart16550::save_state)?,
            ],
            scsi: save_component(
                registry,
                component_ids::SCSI_CONTROLLER,
                PciConfigurationEndpoint::save_state,
            )?,
            parallel: save_component(registry, component_ids::PARALLEL_PORT, Ieee1284::save_state)?,
        })
    }

    /// Rebuilds a complete IP32 machine around a caller-provided trace sink.
    pub fn from_state_with_trace_sink(
        config: Ip32MachineConfig,
        state: Ip32MachineState,
        sink: S,
    ) -> Result<Self, Ip32StateError> {
        if state.schema_version != IP32_STATE_SCHEMA_VERSION {
            return Err(Ip32StateError::UnsupportedSchema {
                version: state.schema_version,
            });
        }
        if Ip32PersistentConfig::from_machine_config(&config) != state.config {
            return Err(Ip32StateError::ConfigurationMismatch);
        }
        let mut machine =
            Self::from_config_with_trace_sink(config, sink).map_err(Ip32StateError::Build)?;
        machine.restore_complete_state(state)?;
        Ok(machine)
    }

    fn restore_complete_state(&mut self, state: Ip32MachineState) -> Result<(), Ip32StateError> {
        let registry = self.runtime.registry_mut();
        restore_component(
            registry,
            component_ids::CPU0,
            state.cpu,
            R5000Cpu::restore_state,
        )?;
        restore_component(
            registry,
            component_ids::CPU_SYSAD_BUS,
            state.sysad,
            Ip32SysAdBus::restore_state,
        )?;
        restore_component(
            registry,
            component_ids::CPU_IRQ_BUS,
            state.cpu_irq,
            IrqBus::restore_state,
        )?;
        restore_component(
            registry,
            component_ids::MACE_IRQ_BUS,
            state.mace_irq,
            IrqBus::restore_state,
        )?;
        restore_component(
            registry,
            component_ids::ONE_WIRE_BUS,
            state.one_wire,
            OneWireBus::restore_state,
        )?;
        let [crt_ddc, flat_panel_ddc] = state.gbe_ddc;
        restore_component(
            registry,
            component_ids::GBE_CRT_DDC_BUS,
            crt_ddc,
            TwoWireBus::restore_state,
        )?;
        restore_component(
            registry,
            component_ids::GBE_FLAT_PANEL_DDC_BUS,
            flat_panel_ddc,
            TwoWireBus::restore_state,
        )?;
        let [keyboard_ps2, mouse_ps2] = state.ps2_buses;
        restore_component(
            registry,
            component_ids::KEYBOARD_PS2_BUS,
            keyboard_ps2,
            TwoWireBus::restore_state,
        )?;
        restore_component(
            registry,
            component_ids::MOUSE_PS2_BUS,
            mouse_ps2,
            TwoWireBus::restore_state,
        )?;
        restore_component(
            registry,
            component_ids::CRIME,
            state.crime,
            Crime::restore_state,
        )?;
        restore_component(
            registry,
            component_ids::CRIME_MEMORY_DOMAIN,
            state.memory_bus,
            CrimeMemoryBus::restore_state,
        )?;
        restore_component(
            registry,
            component_ids::CRIME_MACE_LINK,
            state.cmi,
            CrimeCmiBus::restore_state,
        )?;
        restore_component(
            registry,
            component_ids::CRIME_GBE_LINK,
            state.cgi,
            CrimeCgiBus::restore_state,
        )?;
        restore_component(
            registry,
            component_ids::RAM,
            state.sdram,
            CrimeSdram::restore_state,
        )?;
        restore_component(
            registry,
            component_ids::ISA_BUS,
            state.isa,
            IsaBus::restore_state,
        )?;
        restore_component(
            registry,
            component_ids::PCI_BUS,
            state.pci,
            PciBus::restore_state,
        )?;
        let [i2c0, i2c1] = state.i2c;
        restore_component(
            registry,
            component_ids::I2C_BUS0,
            i2c0,
            I2cBus::restore_state,
        )?;
        restore_component(
            registry,
            component_ids::I2C_BUS1,
            i2c1,
            I2cBus::restore_state,
        )?;
        restore_component(
            registry,
            component_ids::MACE_MEDIA_BUS,
            state.media,
            MediaBus::restore_state,
        )?;
        restore_component(
            registry,
            component_ids::MACE,
            state.mace,
            Mace::restore_state,
        )?;
        restore_component(
            registry,
            component_ids::KEYBOARD,
            state.keyboard,
            Ps2Keyboard::restore_state,
        )?;
        restore_component(
            registry,
            component_ids::MOUSE,
            state.mouse,
            Ps2Mouse::restore_state,
        )?;
        restore_component(registry, component_ids::GBE, state.gbe, Gbe::restore_state)?;
        restore_component(
            registry,
            component_ids::VICE,
            state.vice,
            Ip32StubEndpoint::restore_state,
        )?;
        registry
            .get_typed_mut::<SystemFlash>(component_ids::PROM)
            .map_err(Ip32StateError::Registry)?
            .restore_state(state.system_flash)
            .map_err(Ip32StateError::SystemFlash)?;
        registry
            .get_typed_mut::<Ds1687>(component_ids::RTC)
            .map_err(Ip32StateError::Registry)?
            .restore_state(state.rtc)
            .map_err(Ip32StateError::Rtc)?;
        restore_component(
            registry,
            component_ids::NIC_IDENTITY,
            state.nic_identity,
            Ds2502::restore_state,
        )?;
        let [serial0, serial1] = state.serial;
        restore_component(
            registry,
            component_ids::SERIAL0,
            serial0,
            Uart16550::restore_state,
        )?;
        restore_component(
            registry,
            component_ids::SERIAL1,
            serial1,
            Uart16550::restore_state,
        )?;
        restore_component(
            registry,
            component_ids::SCSI_CONTROLLER,
            state.scsi,
            PciConfigurationEndpoint::restore_state,
        )?;
        restore_component(
            registry,
            component_ids::PARALLEL_PORT,
            state.parallel,
            Ieee1284::restore_state,
        )?;

        self.runtime
            .restore_state(state.runtime)
            .map_err(Ip32StateError::Scheduler)?;
        #[cfg(feature = "jit")]
        let jit_engine = if self.control.jit_engine.is_some() {
            Some(Mips4BlockEngine::new(
                CraneliftMips4Backend::new().map_err(|error| {
                    Ip32StateError::Build(Ip32MachineBuildError::JitInitialization(
                        error.to_string(),
                    ))
                })?,
            ))
        } else {
            None
        };
        #[cfg(feature = "jit")]
        let event_chain_policy = if jit_engine.is_some() {
            Ip32EventChainPolicy::disabled()
        } else {
            Ip32EventChainPolicy {
                sysad: state.control.fusion_sysad,
                memory: state.control.fusion_memory,
                cmi: state.control.fusion_cmi,
                cgi: state.control.fusion_cgi,
                isa: state.control.fusion_isa,
                budget: state.control.fusion_budget,
            }
        };
        #[cfg(not(feature = "jit"))]
        let event_chain_policy = Ip32EventChainPolicy {
            sysad: state.control.fusion_sysad,
            memory: state.control.fusion_memory,
            cmi: state.control.fusion_cmi,
            cgi: state.control.fusion_cgi,
            isa: state.control.fusion_isa,
            budget: state.control.fusion_budget,
        };
        self.control = MachineControl {
            slots: HotComponentSlots::resolve(self.runtime.registry())
                .map_err(Ip32StateError::Registry)?,
            persistent_config: state.config,
            cpu_generation: state.control.cpu_generation,
            cpu_clock: CpuClock {
                frequency_hz: self.control.cpu_clock.frequency_hz,
                remainder: state.control.cpu_clock_remainder,
            },
            host_generation: state.control.host_generation,
            host_capacities: state.control.host_capacities,
            host_reservations: state.control.host_reservations,
            host_outputs: state.control.host_outputs.into(),
            host_output_units: state.control.host_output_units,
            host_dropped_output_bytes: state.control.host_dropped_output_bytes,
            latest_display_frame: state.control.latest_display_frame,
            dropped_display_frames: state.control.dropped_display_frames,
            sysad_transactions: state.control.sysad_transactions,
            memory_transactions: state.control.memory_transactions,
            cmi_transactions: state.control.cmi_transactions,
            cgi_transactions: state.control.cgi_transactions,
            #[cfg(feature = "jit")]
            fast_transaction_attempts: 0,
            #[cfg(feature = "jit")]
            fast_transaction_hits: 0,
            #[cfg(feature = "jit")]
            fast_transaction_fallbacks: 0,
            #[cfg(feature = "jit")]
            jit_code_fetches: Ip32StableCodeFetchStatistics::default(),
            cpu_continuation_quantum: state.control.cpu_continuation_quantum,
            inline_sysad_completion: state.control.inline_sysad_completion,
            event_chain_policy,
            #[cfg(feature = "jit")]
            jit_engine,
            #[cfg(feature = "jit")]
            jit_code_sources: Mips4CodeSourceCache::default(),
            #[cfg(feature = "jit")]
            jit_ram_code_pages: Mips4RamCodePageSet::default(),
            #[cfg(test)]
            capture_logical_transitions: false,
            #[cfg(test)]
            logical_transitions: Vec::new(),
            #[cfg(test)]
            first_serial_output_time: None,
            #[cfg(test)]
            first_cpu_idle_time: None,
        };
        Ok(())
    }
}

/// Failure while applying battery-backed RTC data to an IP32 machine.
#[derive(Debug)]
pub enum RestoreRtcPersistentStateError {
    /// The fixed RTC component was missing or had an unexpected type.
    Registry(RegistryLookupError),
    /// The saved RTC image was malformed.
    Rtc(Ds1687StateError),
}

impl fmt::Display for RestoreRtcPersistentStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registry(error) => error.fmt(formatter),
            Self::Rtc(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for RestoreRtcPersistentStateError {}

/// Failure while applying persistent System Flash data to an IP32 machine.
#[derive(Debug)]
pub enum RestoreSystemFlashPersistentStateError {
    /// The fixed System Flash component was missing or had an unexpected type.
    Registry(RegistryLookupError),
    /// The saved System Flash image was malformed.
    SystemFlash(SystemFlashStateError),
}

impl fmt::Display for RestoreSystemFlashPersistentStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registry(error) => error.fmt(formatter),
            Self::SystemFlash(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for RestoreSystemFlashPersistentStateError {}

fn save_component<T, State>(
    registry: &ComponentRegistry,
    id: ComponentId,
    save: fn(&T) -> State,
) -> Result<State, Ip32StateError>
where
    T: Component,
{
    registry
        .get_typed::<T>(id)
        .map(save)
        .map_err(Ip32StateError::Registry)
}

fn restore_component<T, State>(
    registry: &mut ComponentRegistry,
    id: ComponentId,
    state: State,
    restore: fn(&mut T, State) -> Result<(), ComponentStateError>,
) -> Result<(), Ip32StateError>
where
    T: Component,
{
    restore(
        registry
            .get_typed_mut::<T>(id)
            .map_err(Ip32StateError::Registry)?,
        state,
    )
    .map_err(Ip32StateError::Component)
}

impl<S> Ip32Machine<S>
where
    S: TraceSink,
{
    /// Schedules the power-on event at simulated time zero.
    pub fn schedule_power_on(&mut self) -> Result<ScheduledEventId, SchedulerError> {
        self.runtime
            .schedule_at(SimTime::ZERO, component_ids::MACHINE, Ip32Event::PowerOn)
    }

    /// Schedules a hard-reset event at the current simulated time.
    pub fn schedule_reset(&mut self) -> Result<ScheduledEventId, SchedulerError> {
        self.runtime.schedule_at(
            self.runtime.now(),
            component_ids::MACHINE,
            Ip32Event::HardReset,
        )
    }

    /// Dispatches at most the requested number of IP32 events.
    pub fn run_steps(
        &mut self,
        max_events: usize,
    ) -> Result<RunStatus, RunError<Ip32MachineDispatchError>> {
        let control = &mut self.control;
        self.runtime
            .run_steps(max_events, |event, registry, context| {
                dispatch_event(event, registry, context, control)
            })
    }

    /// Schedules and completely dispatches a hard-reset event.
    pub fn hard_reset(&mut self) -> Result<(), RunError<Ip32MachineDispatchError>> {
        self.runtime.clear_stop();
        let reset_id = self.schedule_reset()?;
        let mut reset_dispatched = false;
        while !reset_dispatched {
            let control = &mut self.control;
            self.runtime.dispatch_next(|event, registry, context| {
                reset_dispatched = event.id == reset_id;
                dispatch_event(event, registry, context, control)
            })?;
        }
        Ok(())
    }

    /// Runs the IP32 machine until the requested simulated-time deadline.
    pub fn run_until_time(
        &mut self,
        deadline: SimTime,
    ) -> Result<RunStatus, RunError<Ip32MachineDispatchError>> {
        let control = &mut self.control;
        self.runtime
            .run_until_time(deadline, |event, registry, context| {
                dispatch_event(event, registry, context, control)
            })
    }
}

fn validate_config(config: &Ip32MachineConfig) -> Result<(), Ip32MachineBuildError> {
    #[cfg(not(feature = "jit"))]
    if config.jit_enabled {
        return Err(Ip32MachineBuildError::JitUnavailable);
    }
    config
        .crime
        .validate()
        .map_err(|error| Ip32MachineBuildError::Crime(CrimeError::Configuration(error)))?;
    if config.prom_image.len() != IP32_PROM_IMAGE_SIZE_BYTES {
        return Err(Ip32MachineBuildError::InvalidPromSize {
            size_bytes: config.prom_image.len(),
        });
    }
    let frequency_hz = config.processor.processor_frequency_hz;
    if !(1..=IP32_TIMEBASE_HZ).contains(&frequency_hz) {
        return Err(Ip32MachineBuildError::InvalidProcessorFrequency { frequency_hz });
    }
    Ok(())
}

fn insert_component(
    registry: &mut ComponentRegistry,
    component: Box<dyn Component>,
) -> Result<(), Ip32MachineBuildError> {
    registry
        .insert(component)
        .map_err(Ip32MachineBuildError::Registry)
}

fn crime_with_trace_interest<'a, S>(
    registry: &'a mut ComponentRegistry,
    context: &Ip32DispatchContext<'_, '_, S>,
    slot: ComponentSlot<Crime>,
) -> Result<&'a mut Crime, RegistryLookupError>
where
    S: TraceSink,
{
    let interest = context.trace_interest(TraceSource::Component(component_ids::CRIME));
    let crime = registry.get_resolved_mut(slot)?;
    crime.set_trace_interest(interest);
    Ok(crime)
}

fn mace_with_trace_interest<'a, S>(
    registry: &'a mut ComponentRegistry,
    context: &Ip32DispatchContext<'_, '_, S>,
    slot: ComponentSlot<Mace>,
) -> Result<&'a mut Mace, RegistryLookupError>
where
    S: TraceSink,
{
    let interest = context.trace_interest(TraceSource::Component(component_ids::MACE));
    let mace = registry.get_resolved_mut(slot)?;
    mace.set_trace_interest(interest);
    Ok(mace)
}

fn dispatch_event<S>(
    event: ScheduledEvent<Ip32Event>,
    registry: &mut ComponentRegistry,
    runtime_context: &mut RuntimeContext<'_, Ip32Event, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    let mut context = Ip32DispatchContext::new(runtime_context, control.event_chain_policy);
    dispatch_event_payload(event.target, event.payload, registry, &mut context, control)?;
    while let Some((target, payload)) = context.take_next_inline()? {
        dispatch_event_payload(target, payload, registry, &mut context, control)?;
    }
    context.finish()?;
    Ok(())
}

fn dispatch_event_payload<S>(
    _target: ComponentId,
    payload: Ip32Event,
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    #[cfg(test)]
    if control.capture_logical_transitions
        && let Some(transition) =
            super::dispatch::logical_transition(context.now(), _target, &payload)
    {
        control.logical_transitions.push(transition);
    }
    context.enter_event(&payload);
    match payload {
        Ip32Event::PowerOn => power_on(registry, context, control)?,
        Ip32Event::HardReset => hard_reset(registry, context, control)?,
        Ip32Event::CpuStep { generation } if generation == control.cpu_generation => {
            dispatch_cpu_step(registry, context, control)?;
        }
        Ip32Event::CpuStep { .. } => {}
        Ip32Event::Crime(event) => {
            crime_with_trace_interest(registry, context, control.slots.crime)?
                .handle_event(context.now(), event);
            drain_crime(registry, context, control)?;
        }
        Ip32Event::SysAdBus(event) => {
            registry
                .get_resolved_mut(control.slots.sysad)?
                .handle_event(event);
            drain_sysad_bus(registry, context, control)?;
        }
        Ip32Event::CrimeMemoryBus(event) => {
            registry
                .get_resolved_mut(control.slots.memory)?
                .handle_event(event);
            drain_memory_bus(registry, context, control)?;
        }
        Ip32Event::CrimeCmiBus(event) => {
            registry
                .get_resolved_mut(control.slots.cmi)?
                .handle_event(event);
            drain_cmi_bus(registry, context, control)?;
        }
        Ip32Event::CrimeCgiBus(event) => {
            registry
                .get_resolved_mut(control.slots.cgi)?
                .handle_event(event);
            drain_cgi_bus(registry, context, control)?;
        }
        Ip32Event::Gbe(event) => {
            let gbe = registry.get_resolved_mut(control.slots.gbe)?;
            gbe.observe_time(context.now());
            gbe.handle_event(event);
            drain_gbe(registry, context, control)?;
        }
        Ip32Event::GbeInput(input) => {
            let gbe = registry.get_resolved_mut(control.slots.gbe)?;
            gbe.observe_time(context.now());
            gbe.apply_external_input(input);
            drain_gbe(registry, context, control)?;
        }
        Ip32Event::Mace(event) => {
            mace_with_trace_interest(registry, context, control.slots.mace)?
                .handle_event(context.now(), event);
            drain_mace(registry, context, control)?;
        }
        Ip32Event::Keyboard(event) => {
            registry
                .get_resolved_mut(control.slots.keyboard)?
                .handle_event(context.now(), event);
            drain_keyboard(registry, context, control)?;
        }
        Ip32Event::Mouse(event) => {
            registry
                .get_resolved_mut(control.slots.mouse)?
                .handle_event(context.now(), event);
            drain_mouse(registry, context, control)?;
        }
        Ip32Event::Ds2502(event) => {
            registry
                .get_resolved_mut(control.slots.nic_identity)?
                .handle_event(context.now(), event);
            drain_ds2502_actions(registry, context, control)?;
            drain_one_wire_bus(registry, context, control)?;
        }
        Ip32Event::IsaBus(event) => {
            registry
                .get_resolved_mut(control.slots.isa)?
                .handle_event(event);
            drain_isa_bus(registry, context, control)?;
        }
        Ip32Event::PciBusService => {
            registry
                .get_typed_mut::<PciBus>(component_ids::PCI_BUS)?
                .service();
            drain_pci_bus(registry, context, control)?;
        }
        Ip32Event::I2cBusService { index } => {
            let bus_id = i2c_bus_id(index)?;
            registry.get_typed_mut::<I2cBus>(bus_id)?.service();
            drain_i2c_bus(registry, context, control, index)?;
        }
        Ip32Event::Uart { port, event } => {
            let uart_id = serial_port_component(port);
            registry
                .get_resolved_mut(control.slots.serial[serial_component_index(uart_id)])?
                .handle_event(event);
            drain_uart(registry, context, control, uart_id)?;
        }
        Ip32Event::HostInput { generation, input } => {
            let units = host_payload_units(&input.payload);
            control.host_reservations[media_port_index(input.port)] =
                control.host_reservations[media_port_index(input.port)].saturating_sub(units);
            if generation != control.host_generation {
                return Ok(());
            }
            let target = match input.port {
                MediaPort::Serial0 | MediaPort::Serial1 => serial_component(input.port),
                _ => component_ids::MACE,
            };
            let transaction = MediaTransaction {
                source: component_ids::MACHINE,
                target,
                port: input.port,
                payload: input.payload,
            };
            registry
                .get_typed_mut::<MediaBus>(component_ids::MACE_MEDIA_BUS)?
                .route(transaction);
            drain_media_bus(registry, context, control)?;
        }
        Ip32Event::Input { generation, input } => {
            if generation != control.host_generation {
                return Ok(());
            }
            match input {
                Ip32InputEvent::Keyboard(input) => {
                    registry
                        .get_resolved_mut(control.slots.keyboard)?
                        .apply_input(input);
                    drain_keyboard(registry, context, control)?;
                }
                Ip32InputEvent::Mouse(input) => {
                    registry
                        .get_resolved_mut(control.slots.mouse)?
                        .apply_input(input);
                    drain_mouse(registry, context, control)?;
                }
                Ip32InputEvent::ReleaseAll => {
                    registry
                        .get_resolved_mut(control.slots.keyboard)?
                        .release_all();
                    registry
                        .get_resolved_mut(control.slots.mouse)?
                        .release_all();
                    drain_keyboard(registry, context, control)?;
                    drain_mouse(registry, context, control)?;
                }
            }
        }
    }
    Ok(())
}

fn media_port_index(port: se_device::bus::media::MediaPort) -> usize {
    use se_device::bus::media::MediaPort;
    match port {
        MediaPort::VideoInputAb => 0,
        MediaPort::VideoInputCd => 1,
        MediaPort::VideoOutput => 2,
        MediaPort::AudioInput => 3,
        MediaPort::AudioOutput1 => 4,
        MediaPort::AudioOutput2 => 5,
        MediaPort::Ethernet => 6,
        MediaPort::Keyboard => 7,
        MediaPort::Mouse => 8,
        MediaPort::Serial0 => 9,
        MediaPort::Serial1 => 10,
        MediaPort::Parallel => 11,
    }
}

fn host_payload_units(payload: &MediaPayload) -> usize {
    match payload {
        MediaPayload::Bytes(bytes) => bytes.len(),
        MediaPayload::Ethernet(frame) => frame.data.len(),
        MediaPayload::Audio(block) => block.samples.len(),
        MediaPayload::Video(field) => field.data.len(),
        MediaPayload::Sync { .. } => 1,
    }
}

fn serial_component(port: MediaPort) -> ComponentId {
    match port {
        MediaPort::Serial0 => component_ids::SERIAL0,
        MediaPort::Serial1 => component_ids::SERIAL1,
        _ => unreachable!("non-serial media port has no UART component"),
    }
}

fn serial_port_component(port: Ip32SerialPort) -> ComponentId {
    match port {
        Ip32SerialPort::Serial1 => component_ids::SERIAL0,
        Ip32SerialPort::Serial2 => component_ids::SERIAL1,
    }
}

fn serial_port_for_component(component: ComponentId) -> Ip32SerialPort {
    if component == component_ids::SERIAL0 {
        Ip32SerialPort::Serial1
    } else {
        Ip32SerialPort::Serial2
    }
}

fn serial_component_index(component: ComponentId) -> usize {
    usize::from(component == component_ids::SERIAL1)
}

fn media_port_for_serial_component(component: ComponentId) -> MediaPort {
    if component == component_ids::SERIAL0 {
        MediaPort::Serial0
    } else {
        MediaPort::Serial1
    }
}

fn media_port_capacity(
    capacities: MacePortConfig,
    port: se_device::bus::media::MediaPort,
) -> usize {
    use se_device::bus::media::MediaPort;
    match port {
        MediaPort::Ethernet => capacities.ethernet_frames,
        MediaPort::AudioInput | MediaPort::AudioOutput1 | MediaPort::AudioOutput2 => {
            capacities.audio_sample_pairs
        }
        MediaPort::VideoInputAb | MediaPort::VideoInputCd | MediaPort::VideoOutput => {
            capacities.video_fields
        }
        _ => capacities.byte_stream_bytes,
    }
}

fn power_on<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    advance_cpu_generation(control)?;
    advance_host_generation(control)?;
    reset_jit_engine(control)?;
    control.host_outputs.clear();
    control.host_output_units.fill(0);
    control.host_dropped_output_bytes.fill(0);
    control.latest_display_frame = None;
    control.dropped_display_frames = 0;
    registry
        .get_typed_mut::<R5000Cpu>(component_ids::CPU0)?
        .reset();
    registry
        .get_typed_mut::<IrqBus>(component_ids::CPU_IRQ_BUS)?
        .reset();
    registry
        .get_typed_mut::<IrqBus>(component_ids::MACE_IRQ_BUS)?
        .reset();
    registry
        .get_typed_mut::<Ip32SysAdBus>(component_ids::CPU_SYSAD_BUS)?
        .hard_reset();
    registry
        .get_typed_mut::<CrimeCmiBus>(component_ids::CRIME_MACE_LINK)?
        .hard_reset();
    registry
        .get_typed_mut::<CrimeCgiBus>(component_ids::CRIME_GBE_LINK)?
        .hard_reset();
    registry.get_resolved_mut(control.slots.isa)?.hard_reset();
    registry
        .get_typed_mut::<PciBus>(component_ids::PCI_BUS)?
        .reset();
    registry
        .get_typed_mut::<I2cBus>(component_ids::I2C_BUS0)?
        .reset();
    registry
        .get_typed_mut::<I2cBus>(component_ids::I2C_BUS1)?
        .reset();
    registry
        .get_typed_mut::<MediaBus>(component_ids::MACE_MEDIA_BUS)?
        .reset();
    registry.get_resolved_mut(control.slots.one_wire)?.reset();
    registry.get_resolved_mut(control.slots.gbe_ddc[0])?.reset();
    registry.get_resolved_mut(control.slots.gbe_ddc[1])?.reset();
    registry
        .get_resolved_mut(control.slots.ps2_buses[0])?
        .reset();
    registry
        .get_resolved_mut(control.slots.ps2_buses[1])?
        .reset();
    registry
        .get_resolved_mut(control.slots.nic_identity)?
        .power_on(context.now());
    mace_with_trace_interest(registry, context, control.slots.mace)?.power_on(context.now());
    registry
        .get_resolved_mut(control.slots.keyboard)?
        .power_on(context.now());
    registry
        .get_resolved_mut(control.slots.mouse)?
        .power_on(context.now());
    registry
        .get_resolved_mut(control.slots.rtc)?
        .power_on(context.now());
    registry.get_resolved_mut(control.slots.serial[0])?.reset();
    registry.get_resolved_mut(control.slots.serial[1])?.reset();
    registry
        .get_typed_mut::<PciConfigurationEndpoint>(component_ids::SCSI_CONTROLLER)?
        .reset();
    registry.get_resolved_mut(control.slots.parallel)?.reset();
    registry.get_resolved_mut(control.slots.gbe)?.reset();
    registry
        .get_typed_mut::<Ip32StubEndpoint>(component_ids::VICE)?
        .reset();
    registry
        .get_typed_mut::<CrimeMemoryBus>(component_ids::CRIME_MEMORY_DOMAIN)?
        .power_on(context.now());
    crime_with_trace_interest(registry, context, control.slots.crime)?.power_on(context.now());
    drain_crime(registry, context, control)?;
    drain_mace(registry, context, control)?;
    drain_keyboard(registry, context, control)?;
    drain_mouse(registry, context, control)?;
    context.schedule_after(
        SimDuration::new(SDRAM_INITIALIZATION_TICKS),
        component_ids::CPU0,
        Ip32Event::CpuStep {
            generation: control.cpu_generation,
        },
    )?;
    Ok(())
}

fn hard_reset<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    advance_cpu_generation(control)?;
    advance_host_generation(control)?;
    reset_jit_engine(control)?;
    control.latest_display_frame = None;
    control.dropped_display_frames = 0;
    registry
        .get_typed_mut::<R5000Cpu>(component_ids::CPU0)?
        .reset();
    registry
        .get_typed_mut::<IrqBus>(component_ids::CPU_IRQ_BUS)?
        .reset();
    registry
        .get_typed_mut::<IrqBus>(component_ids::MACE_IRQ_BUS)?
        .reset();
    registry
        .get_typed_mut::<Ip32SysAdBus>(component_ids::CPU_SYSAD_BUS)?
        .hard_reset();
    registry
        .get_typed_mut::<CrimeCmiBus>(component_ids::CRIME_MACE_LINK)?
        .hard_reset();
    registry
        .get_typed_mut::<CrimeCgiBus>(component_ids::CRIME_GBE_LINK)?
        .hard_reset();
    registry.get_resolved_mut(control.slots.isa)?.hard_reset();
    registry
        .get_typed_mut::<PciBus>(component_ids::PCI_BUS)?
        .reset();
    registry
        .get_typed_mut::<I2cBus>(component_ids::I2C_BUS0)?
        .reset();
    registry
        .get_typed_mut::<I2cBus>(component_ids::I2C_BUS1)?
        .reset();
    registry
        .get_typed_mut::<MediaBus>(component_ids::MACE_MEDIA_BUS)?
        .reset();
    registry.get_resolved_mut(control.slots.one_wire)?.reset();
    registry.get_resolved_mut(control.slots.gbe_ddc[0])?.reset();
    registry.get_resolved_mut(control.slots.gbe_ddc[1])?.reset();
    registry
        .get_resolved_mut(control.slots.ps2_buses[0])?
        .reset();
    registry
        .get_resolved_mut(control.slots.ps2_buses[1])?
        .reset();
    registry
        .get_resolved_mut(control.slots.nic_identity)?
        .hard_reset(context.now());
    mace_with_trace_interest(registry, context, control.slots.mace)?.hard_reset(context.now());
    registry
        .get_resolved_mut(control.slots.keyboard)?
        .power_on(context.now());
    registry
        .get_resolved_mut(control.slots.mouse)?
        .power_on(context.now());
    registry
        .get_resolved_mut(control.slots.rtc)?
        .hard_reset(context.now());
    registry.get_resolved_mut(control.slots.serial[0])?.reset();
    registry.get_resolved_mut(control.slots.serial[1])?.reset();
    registry
        .get_typed_mut::<PciConfigurationEndpoint>(component_ids::SCSI_CONTROLLER)?
        .reset();
    registry.get_resolved_mut(control.slots.parallel)?.reset();
    registry.get_resolved_mut(control.slots.gbe)?.reset();
    registry
        .get_typed_mut::<Ip32StubEndpoint>(component_ids::VICE)?
        .reset();
    registry
        .get_typed_mut::<CrimeMemoryBus>(component_ids::CRIME_MEMORY_DOMAIN)?
        .hard_reset(context.now());
    crime_with_trace_interest(registry, context, control.slots.crime)?.hard_reset(context.now());
    drain_crime(registry, context, control)?;
    drain_mace(registry, context, control)?;
    drain_keyboard(registry, context, control)?;
    drain_mouse(registry, context, control)?;
    context.schedule_at(
        context.now(),
        component_ids::CPU0,
        Ip32Event::CpuStep {
            generation: control.cpu_generation,
        },
    )?;
    Ok(())
}

fn warm_reset<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    advance_cpu_generation(control)?;
    registry
        .get_typed_mut::<R5000Cpu>(component_ids::CPU0)?
        .accept(R5000CpuSignal::SoftReset);
    registry
        .get_typed_mut::<Ip32SysAdBus>(component_ids::CPU_SYSAD_BUS)?
        .hard_reset();
    crime_with_trace_interest(registry, context, control.slots.crime)?.warm_reset();
    context.schedule_at(
        context.now(),
        component_ids::CPU0,
        Ip32Event::CpuStep {
            generation: control.cpu_generation,
        },
    )?;
    Ok(())
}

fn advance_cpu_generation(control: &mut MachineControl) -> Result<(), Ip32MachineDispatchError> {
    control.cpu_generation = control
        .cpu_generation
        .checked_add(1)
        .ok_or(Ip32MachineDispatchError::GenerationOverflow)?;
    control.cpu_clock.reset();
    Ok(())
}

#[cfg(feature = "jit")]
fn reset_jit_engine(control: &mut MachineControl) -> Result<(), Ip32MachineDispatchError> {
    clear_jit_code_sources(control);
    if let Some(engine) = &mut control.jit_engine {
        engine.reset().map_err(|error| {
            Ip32MachineDispatchError::Cpu(R5000CpuError::Block(error.to_string()))
        })?;
    }
    Ok(())
}

#[cfg(feature = "jit")]
fn clear_jit_code_sources(control: &mut MachineControl) {
    control.jit_code_sources.clear();
    control.jit_ram_code_pages.clear();
}

#[cfg(feature = "jit")]
fn remember_ram_code_source(control: &mut MachineControl, address: u64, length: usize) {
    let Some(last_address) = address.checked_add(length.saturating_sub(1) as u64) else {
        return;
    };
    let first_page = address >> MIPS4_RAM_CODE_PAGE_SHIFT;
    let last_page = last_address >> MIPS4_RAM_CODE_PAGE_SHIFT;
    for page in first_page..=last_page {
        control.jit_ram_code_pages.insert(page);
    }
}

#[cfg(feature = "jit")]
fn invalidate_ram_code_sources(control: &mut MachineControl, range: Option<(u64, usize)>) {
    let Some((address, length)) = range else {
        control
            .jit_code_sources
            .retain(|key, _| key.source != Ip32StableCodeSource::Sdram);
        control.jit_ram_code_pages.clear();
        return;
    };
    if length == 0 {
        return;
    }
    let write_end = address.saturating_add(length as u64);
    let last_address = write_end.saturating_sub(1);
    let first_page = address >> MIPS4_RAM_CODE_PAGE_SHIFT;
    let last_page = last_address >> MIPS4_RAM_CODE_PAGE_SHIFT;
    if !(first_page..=last_page).any(|page| control.jit_ram_code_pages.contains(&page)) {
        return;
    }
    control.jit_code_sources.retain(|key, _| {
        if key.source != Ip32StableCodeSource::Sdram {
            return true;
        }
        let source_end = key.source_offset.saturating_add(u64::from(key.byte_count));
        write_end <= key.source_offset || source_end <= address
    });
    control.jit_ram_code_pages.clear();
    let remaining = control
        .jit_code_sources
        .keys()
        .filter(|key| key.source == Ip32StableCodeSource::Sdram)
        .map(|key| (key.source_offset, usize::from(key.byte_count)))
        .collect::<Vec<_>>();
    for (source_offset, byte_count) in remaining {
        remember_ram_code_source(control, source_offset, byte_count);
    }
}

#[cfg(not(feature = "jit"))]
fn reset_jit_engine(_control: &mut MachineControl) -> Result<(), Ip32MachineDispatchError> {
    Ok(())
}

fn advance_host_generation(control: &mut MachineControl) -> Result<(), Ip32MachineDispatchError> {
    control.host_generation = control
        .host_generation
        .checked_add(1)
        .ok_or(Ip32MachineDispatchError::GenerationOverflow)?;
    control.host_reservations.fill(0);
    Ok(())
}

fn dispatch_cpu_step<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    #[cfg(feature = "jit")]
    if control.jit_engine.is_some() {
        return drive_cpu_jit(registry, context, control);
    }
    let action = registry.get_resolved_mut(control.slots.cpu)?.poll()?;
    drive_cpu(registry, context, control, action)
}

#[cfg(feature = "jit")]
fn plan_stable_timeline<S>(
    context: &Ip32DispatchContext<'_, '_, S>,
    control: &MachineControl,
    maximum_fetches: usize,
    clocks: &[Mips4SliceClock],
    fixed_ticks_per_fetch: u64,
    stable_deadline: Option<SimTime>,
) -> Option<Mips4SliceTimeline>
where
    S: TraceSink,
{
    let timeline = Mips4SliceTimeline::new(maximum_fetches, clocks, fixed_ticks_per_fetch)?;
    let pclock = control.cpu_clock.projection();
    let fits = |candidate: usize| {
        let fetch_ticks = timeline.prefix_ticks(candidate)?;
        let pclock_ticks = pclock.elapsed(candidate as u64)?.get();
        let total = fetch_ticks.checked_add(pclock_ticks)?;
        Some(
            context
                .now()
                .checked_add(SimDuration::new(total))
                .is_some_and(|end| {
                    end <= context.deadline()
                        && context.next_event_time().is_none_or(|event| event > end)
                        && stable_deadline.is_none_or(|deadline| end <= deadline)
                }),
        )
    };
    if fits(maximum_fetches)? {
        return Some(timeline);
    }
    let mut lower = 0;
    let mut upper = maximum_fetches - 1;
    while lower < upper {
        let candidate = lower + (upper - lower).div_ceil(2);
        if fits(candidate)? {
            lower = candidate;
        } else {
            upper = candidate - 1;
        }
    }
    Mips4SliceTimeline::new(lower, clocks, fixed_ticks_per_fetch)
}

#[cfg(feature = "jit")]
fn stable_prom_code_window<S>(
    registry: &ComponentRegistry,
    context: &Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
    maximum_instructions: usize,
) -> Result<Option<Mips4CodeWindow>, Ip32MachineDispatchError>
where
    S: TraceSink,
{
    if maximum_instructions == 0
        || !registry
            .get_resolved(control.slots.sysad)?
            .stable_fetch_ready()
        || !registry
            .get_resolved(control.slots.cmi)?
            .stable_fetch_ready()
        || !registry
            .get_resolved(control.slots.isa)?
            .stable_fetch_ready()
        || !registry
            .get_resolved(control.slots.crime)?
            .stable_cpu_fetch_ready()
        || !registry
            .get_resolved(control.slots.mace)?
            .stable_prom_fetch_ready()
    {
        return Ok(None);
    }
    let Some(request) = registry
        .get_resolved(control.slots.cpu)?
        .code_source_request()
    else {
        return Ok(None);
    };
    let ram_size = registry
        .get_resolved(control.slots.sdram)?
        .total_size_bytes();
    let Ip32AddressResolution::Memory {
        region: Ip32PhysicalRegion::SystemRom,
        offset,
        ..
    } = super::address_map::resolve(request.physical_address, 4, ram_size)
    else {
        return Ok(None);
    };

    let flash = registry.get_resolved(control.slots.prom)?;
    let available = flash.len().saturating_sub(offset as usize);
    let source_instructions = (usize::from(request.maximum_bytes) / 4)
        .min(available / 4)
        .min(MIPS4_BLOCK_MAX_INSTRUCTIONS);
    let requested_instructions = maximum_instructions.min(source_instructions);
    if requested_instructions == 0 {
        return Ok(None);
    }
    let sysad = registry
        .get_resolved(control.slots.sysad)?
        .stable_fetch_clock()
        .and_then(|clock| Mips4SliceClock::new(clock, 2))
        .expect("stable SysAD readiness was checked");
    let cmi = registry
        .get_resolved(control.slots.cmi)?
        .stable_fetch_clock()
        .and_then(|clock| Mips4SliceClock::new(clock, 2))
        .expect("stable CMI readiness was checked");
    let isa = registry
        .get_resolved(control.slots.isa)?
        .stable_fetch_delay()
        .expect("stable ISA readiness was checked")
        .get();
    let Some(timeline) = plan_stable_timeline(
        context,
        control,
        requested_instructions,
        &[sysad, cmi],
        isa,
        None,
    ) else {
        return Ok(None);
    };

    let byte_count = source_instructions * 4;
    let revision = flash.persistence_revision();
    let cache_key = Mips4CodeSourceCacheKey {
        source: Ip32StableCodeSource::SystemFlash,
        source_offset: offset,
        revision,
        byte_count: byte_count as u8,
        no_ecc: false,
    };
    if control.jit_code_sources.len() == MIPS4_BLOCK_CACHE_CAPACITY
        && !control.jit_code_sources.contains_key(&cache_key)
    {
        clear_jit_code_sources(control);
    }
    let source = control
        .jit_code_sources
        .entry(cache_key)
        .or_insert_with(|| {
            let bytes = flash.bytes()[offset as usize..offset as usize + byte_count].to_vec();
            let fingerprint = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
                (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
            });
            Mips4CodeSourceCacheEntry { bytes, fingerprint }
        });
    let guard = Mips4CodeGuard {
        source_id: Ip32StableCodeSource::SystemFlash.id(),
        source_offset: offset,
        revision,
        fingerprint: source.fingerprint,
    };
    Ok(Mips4CodeWindow::new(
        request,
        guard,
        &source.bytes,
        timeline,
    ))
}

#[cfg(feature = "jit")]
fn stable_ram_code_window<S>(
    registry: &ComponentRegistry,
    context: &Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
    maximum_instructions: usize,
) -> Result<Option<Mips4CodeWindow>, Ip32MachineDispatchError>
where
    S: TraceSink,
{
    let memory = registry.get_resolved(control.slots.memory)?;
    let refresh_deadline = memory.stable_fetch_refresh_deadline();
    if maximum_instructions == 0
        || !registry
            .get_resolved(control.slots.sysad)?
            .stable_fetch_ready()
        || !memory.stable_fetch_ready()
        || refresh_deadline.is_some_and(|deadline| context.now() >= deadline)
        || !registry
            .get_resolved(control.slots.crime)?
            .stable_cpu_fetch_ready()
    {
        return Ok(None);
    }
    let Some(request) = registry
        .get_resolved(control.slots.cpu)?
        .code_source_request()
    else {
        return Ok(None);
    };
    let ram_size = registry
        .get_resolved(control.slots.sdram)?
        .total_size_bytes();
    let Ip32AddressResolution::Memory {
        region:
            Ip32PhysicalRegion::LowMemory
            | Ip32PhysicalRegion::HighMemoryUnconfirmed
            | Ip32PhysicalRegion::LinearMemory
            | Ip32PhysicalRegion::NoEccMemory,
        offset,
        no_ecc,
        ..
    } = super::address_map::resolve(request.physical_address, 4, ram_size)
    else {
        return Ok(None);
    };
    let source_instructions =
        (usize::from(request.maximum_bytes) / 4).min(MIPS4_BLOCK_MAX_INSTRUCTIONS);
    let requested_instructions = maximum_instructions.min(source_instructions);
    if requested_instructions == 0 {
        return Ok(None);
    }
    let sysad = registry
        .get_resolved(control.slots.sysad)?
        .stable_fetch_clock()
        .and_then(|clock| Mips4SliceClock::new(clock, 2))
        .expect("stable SysAD readiness was checked");
    let memory = registry
        .get_resolved(control.slots.memory)?
        .stable_fetch_clock()
        .and_then(|clock| Mips4SliceClock::new(clock, 2))
        .expect("stable memory-bus readiness was checked");
    let Some(timeline) = plan_stable_timeline(
        context,
        control,
        requested_instructions,
        &[sysad, memory],
        0,
        refresh_deadline,
    ) else {
        return Ok(None);
    };
    let byte_count = source_instructions * 4;
    let cache_key = Mips4CodeSourceCacheKey {
        source: Ip32StableCodeSource::Sdram,
        source_offset: offset,
        revision: 0,
        byte_count: byte_count as u8,
        no_ecc,
    };
    if control.jit_code_sources.len() == MIPS4_BLOCK_CACHE_CAPACITY
        && !control.jit_code_sources.contains_key(&cache_key)
    {
        clear_jit_code_sources(control);
    }
    let window = {
        let source = match control.jit_code_sources.entry(cache_key) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let Some((bytes, fingerprint)) = registry
                    .get_resolved(control.slots.sdram)?
                    .stable_code_window(offset, byte_count, no_ecc)
                else {
                    return Ok(None);
                };
                entry.insert(Mips4CodeSourceCacheEntry { bytes, fingerprint })
            }
        };
        let guard = Mips4CodeGuard {
            source_id: Ip32StableCodeSource::Sdram.id(),
            source_offset: offset,
            revision: 0,
            fingerprint: source.fingerprint,
        };
        Mips4CodeWindow::new(request, guard, &source.bytes, timeline)
    };
    if window.is_some() {
        remember_ram_code_source(control, offset, byte_count);
    }
    Ok(window)
}

#[cfg(feature = "jit")]
fn stable_code_window<S>(
    registry: &ComponentRegistry,
    context: &Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
    maximum_instructions: usize,
) -> Result<Option<Mips4CodeWindow>, Ip32MachineDispatchError>
where
    S: TraceSink,
{
    if registry
        .get_resolved(control.slots.cpu)?
        .code_source_request()
        .is_none()
    {
        return Ok(None);
    }
    if let Some(window) = stable_prom_code_window(registry, context, control, maximum_instructions)?
    {
        return Ok(Some(window));
    }
    stable_ram_code_window(registry, context, control, maximum_instructions)
}

#[cfg(feature = "jit")]
fn commit_stable_prom_fetches(
    registry: &mut ComponentRegistry,
    control: &mut MachineControl,
    window: &Mips4CodeWindow,
    fetches: usize,
    slice_start: SimTime,
) -> Result<(), Ip32MachineDispatchError> {
    if fetches == 0 {
        return Ok(());
    }
    let previous_fetches = fetches - 1;
    let previous_fetch_ticks = window
        .fetch_time_ticks(previous_fetches)
        .expect("executed fetches must fit the stable timeline");
    let previous_pclock_ticks = control
        .cpu_clock
        .projection()
        .elapsed(previous_fetches as u64)
        .expect("the bounded slice PClock projection cannot overflow")
        .get();
    let previous_ticks = previous_fetch_ticks
        .checked_add(previous_pclock_ticks)
        .expect("the bounded slice timeline cannot overflow");
    let previous_time = slice_start
        .checked_add(SimDuration::new(previous_ticks))
        .expect("the bounded slice timeline must fit simulated time");
    let sysad = registry
        .get_resolved(control.slots.sysad)?
        .stable_fetch_clock()
        .expect("stable SysAD readiness was checked");
    let cmi = registry
        .get_resolved(control.slots.cmi)?
        .stable_fetch_clock()
        .expect("stable CMI readiness was checked");
    let previous_bus_cycles = (previous_fetches as u64)
        .checked_mul(2)
        .expect("the bounded slice cycle count cannot overflow");
    let sysad_delivery_delay = fractional_cycle_delay(sysad, previous_bus_cycles);
    let cmi_delivery_delay = fractional_cycle_delay(cmi, previous_bus_cycles);
    let crime_delivery_time = previous_time
        .checked_add(sysad_delivery_delay)
        .expect("the bounded SysAD delivery time must fit simulated time");
    let mace_delivery_time = crime_delivery_time
        .checked_add(cmi_delivery_delay)
        .expect("the bounded CMI delivery time must fit simulated time");
    registry
        .get_resolved_mut(control.slots.sysad)?
        .commit_stable_fetches(fetches);
    registry
        .get_resolved_mut(control.slots.cmi)?
        .commit_stable_fetches(fetches);
    if !registry
        .get_resolved_mut(control.slots.crime)?
        .account_stable_cpu_fetches(fetches, crime_delivery_time)?
        || !registry
            .get_resolved_mut(control.slots.mace)?
            .account_stable_prom_fetches(fetches, mace_delivery_time)
    {
        return Err(Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
            "stable PROM fetch lost idle bus ownership".to_owned(),
        )));
    }
    control
        .jit_code_fetches
        .record(Ip32StableCodeSource::SystemFlash, fetches as u64);
    Ok(())
}

#[cfg(feature = "jit")]
fn commit_stable_ram_fetches(
    registry: &mut ComponentRegistry,
    control: &mut MachineControl,
    window: &Mips4CodeWindow,
    fetches: usize,
    slice_start: SimTime,
) -> Result<(), Ip32MachineDispatchError> {
    if fetches == 0 {
        return Ok(());
    }
    let previous_fetches = fetches - 1;
    let previous_fetch_ticks = window
        .fetch_time_ticks(previous_fetches)
        .expect("executed fetches must fit the stable timeline");
    let previous_pclock_ticks = control
        .cpu_clock
        .projection()
        .elapsed(previous_fetches as u64)
        .expect("the bounded slice PClock projection cannot overflow")
        .get();
    let previous_ticks = previous_fetch_ticks
        .checked_add(previous_pclock_ticks)
        .expect("the bounded slice timeline cannot overflow");
    let previous_time = slice_start
        .checked_add(SimDuration::new(previous_ticks))
        .expect("the bounded slice timeline must fit simulated time");
    let sysad = registry
        .get_resolved(control.slots.sysad)?
        .stable_fetch_clock()
        .expect("stable SysAD readiness was checked");
    let previous_bus_cycles = (previous_fetches as u64)
        .checked_mul(2)
        .expect("the bounded slice cycle count cannot overflow");
    let crime_delivery_time = previous_time
        .checked_add(fractional_cycle_delay(sysad, previous_bus_cycles))
        .expect("the bounded SysAD delivery time must fit simulated time");
    registry
        .get_resolved_mut(control.slots.sysad)?
        .commit_stable_fetches(fetches);
    registry
        .get_resolved_mut(control.slots.memory)?
        .commit_stable_fetches(fetches);
    if !registry
        .get_resolved_mut(control.slots.crime)?
        .account_stable_cpu_fetches(fetches, crime_delivery_time)?
    {
        return Err(Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
            "stable RAM fetch lost idle bus ownership".to_owned(),
        )));
    }
    control
        .jit_code_fetches
        .record(Ip32StableCodeSource::Sdram, fetches as u64);
    Ok(())
}

#[cfg(feature = "jit")]
fn fractional_cycle_delay(
    projection: FractionalClockProjection,
    previous_cycles: u64,
) -> SimDuration {
    let before = projection
        .elapsed(previous_cycles)
        .expect("the bounded clock projection cannot overflow")
        .get();
    let after = projection
        .elapsed(previous_cycles + 1)
        .expect("the bounded clock projection cannot overflow")
        .get();
    SimDuration::new(after - before)
}

#[cfg(feature = "jit")]
#[inline(never)]
fn drive_cpu_jit<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    let mut boundaries = 0;
    let fast_transaction_tracing_disabled = fast_transaction_tracing_disabled(context);
    loop {
        let slice_start = context.now();
        let remaining = control.cpu_continuation_quantum - boundaries;
        let planned = control.cpu_clock.plan_boundary_budget(
            context.now(),
            context.deadline(),
            context.next_event_time(),
            remaining,
        );
        let mut fast_memory_runtime = plan_fast_memory_runtime(
            registry,
            context,
            control,
            fast_transaction_tracing_disabled,
        )?;
        let (requested, cached_slice) = {
            let engine = control
                .jit_engine
                .as_mut()
                .expect("JIT dispatch requires an initialized engine");
            let cpu = registry.get_resolved_mut(control.slots.cpu)?;
            let requested = cpu.limit_slice_budget(planned as u64);
            let cached_slice = match fast_memory_runtime.as_mut() {
                Some(runtime) => {
                    cpu.run_reusable_slice_with_fast_memory(engine, requested, runtime)?
                }
                None => cpu.run_reusable_slice(engine, requested)?,
            };
            (requested, cached_slice)
        };
        let (code_window, slice) = if let Some(slice) = cached_slice {
            (None, slice)
        } else {
            let code_window = if fast_transaction_tracing_disabled {
                stable_code_window(registry, context, control, requested as usize)?
            } else {
                None
            };
            if let (Some(runtime), Some(window)) =
                (fast_memory_runtime.as_mut(), code_window.as_ref())
            {
                configure_fast_memory_code_timeline(registry, control, runtime, window)?;
            }
            let requested = code_window.as_ref().map_or(requested, |window| {
                requested.min(window.fetch_count() as u64)
            });
            let engine = control
                .jit_engine
                .as_mut()
                .expect("JIT dispatch requires an initialized engine");
            let cpu = registry.get_resolved_mut(control.slots.cpu)?;
            let slice = match fast_memory_runtime.as_mut() {
                Some(runtime) => cpu.run_slice_with_code_window_and_fast_memory(
                    engine,
                    requested,
                    code_window.as_ref(),
                    runtime,
                )?,
                None => match code_window.as_ref() {
                    Some(window) => {
                        cpu.run_slice_with_code_window(engine, requested, Some(window))?
                    }
                    None => cpu.run_slice(engine, requested)?,
                },
            };
            (code_window, slice)
        };

        let combined_code_timeline = fast_memory_runtime
            .as_ref()
            .is_some_and(|runtime| runtime.context.code_fetch_active());
        if let Some(runtime) = &fast_memory_runtime {
            commit_fast_memory_runtime(
                registry,
                context,
                control,
                runtime,
                code_window.as_ref(),
                slice.fast_fetches,
            )?;
        }

        if slice.simulated_time_ticks != 0 && !combined_code_timeline {
            let delay = SimDuration::new(slice.simulated_time_ticks);
            let next_time =
                context
                    .now()
                    .checked_add(delay)
                    .ok_or(SchedulerError::TimeOverflow {
                        time: context.now(),
                        duration: delay,
                    })?;
            if !context.try_advance_to(next_time)? {
                return Err(Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
                    "stable PROM timeline crossed a scheduler event".to_owned(),
                )));
            }
            let window = code_window
                .as_ref()
                .expect("fast fetches require a code window");
            match Ip32StableCodeSource::from_guard(window.guard())? {
                Ip32StableCodeSource::SystemFlash => commit_stable_prom_fetches(
                    registry,
                    control,
                    window,
                    slice.fast_fetches as usize,
                    slice_start,
                )?,
                Ip32StableCodeSource::Sdram => commit_stable_ram_fetches(
                    registry,
                    control,
                    window,
                    slice.fast_fetches as usize,
                    slice_start,
                )?,
            }
        }

        let slice_boundaries = usize::try_from(slice.boundaries)
            .expect("a bounded JIT slice boundary count must fit usize");
        let bulk_boundaries = slice_boundaries.saturating_sub(1);
        if bulk_boundaries != 0 {
            let delay = control.cpu_clock.advance_pclocks(bulk_boundaries as u64);
            let next_time =
                context
                    .now()
                    .checked_add(delay)
                    .ok_or(SchedulerError::TimeOverflow {
                        time: context.now(),
                        duration: delay,
                    })?;
            if !context.try_advance_to(next_time)? {
                return Err(Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
                    "planned PClock prefix crossed a scheduler event".to_owned(),
                )));
            }
            boundaries += bulk_boundaries;
        }

        for _ in bulk_boundaries..slice_boundaries {
            boundaries += 1;
            let delay = control.cpu_clock.next_pclock_delay();
            if boundaries >= control.cpu_continuation_quantum {
                schedule_cpu_step(context, control, delay)?;
                return Ok(());
            }
            let next_time =
                context
                    .now()
                    .checked_add(delay)
                    .ok_or(SchedulerError::TimeOverflow {
                        time: context.now(),
                        duration: delay,
                    })?;
            if !context.try_advance_to(next_time)? {
                schedule_cpu_step(context, control, delay)?;
                return Ok(());
            }
        }

        match slice.action {
            R5000ExecutionSliceAction::Transaction(transaction) => {
                if try_complete_fast_cpu_transaction(
                    registry,
                    context,
                    control,
                    &transaction,
                    fast_transaction_tracing_disabled,
                )? {
                    continue;
                }
                return route_cpu_transaction(registry, context, control, transaction);
            }
            R5000ExecutionSliceAction::Progress if slice.boundaries != 0 => {}
            R5000ExecutionSliceAction::Progress => {
                return Err(Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
                    "JIT slice made no architectural progress".to_owned(),
                )));
            }
            R5000ExecutionSliceAction::Idle => {
                #[cfg(test)]
                control.first_cpu_idle_time.get_or_insert(context.now());
                return Ok(());
            }
            R5000ExecutionSliceAction::Waiting { .. } => {
                return Ok(());
            }
        }
    }
}

#[cfg(feature = "jit")]
fn try_complete_fast_cpu_transaction<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
    transaction: &ExecutionTransaction<Mips4ExecutionTransaction>,
    tracing_disabled: bool,
) -> Result<bool, Ip32MachineDispatchError>
where
    S: TraceSink,
{
    control.fast_transaction_attempts = control.fast_transaction_attempts.saturating_add(1);
    let Some(plan) =
        plan_fast_cpu_transaction(registry, context, control, transaction, tracing_disabled)?
    else {
        control.fast_transaction_fallbacks = control.fast_transaction_fallbacks.saturating_add(1);
        return Ok(false);
    };
    if !context.try_advance_to(plan.completion_time)? {
        control.fast_transaction_fallbacks = control.fast_transaction_fallbacks.saturating_add(1);
        return Ok(false);
    }
    if !registry
        .get_resolved_mut(control.slots.sysad)?
        .commit_direct_transaction(plan.sysad)
    {
        return Err(fast_transaction_invariant(
            "the SysAD transaction changed after planning",
        ));
    }
    let payload = match plan.route {
        Ip32CpuTransactionRoute::InternalRegister => registry
            .get_resolved_mut(control.slots.crime)?
            .commit_synchronous_sysad_read(transaction.payload, plan.delivery_time),
        Ip32CpuTransactionRoute::Memory {
            plan: memory_plan,
            transaction: memory_transaction,
        } => commit_fast_cpu_memory_transaction(
            registry,
            control,
            transaction,
            plan.delivery_time,
            memory_plan,
            memory_transaction,
        )?,
    };
    registry
        .get_resolved_mut(control.slots.cpu)?
        .complete(ExecutionCompletion {
            id: transaction.id,
            payload,
        });
    control.sysad_transactions = control.sysad_transactions.saturating_add(1);
    control.fast_transaction_hits = control.fast_transaction_hits.saturating_add(1);
    Ok(true)
}

#[cfg(feature = "jit")]
fn commit_fast_cpu_memory_transaction(
    registry: &mut ComponentRegistry,
    control: &mut MachineControl,
    cpu_transaction: &ExecutionTransaction<Mips4ExecutionTransaction>,
    delivery_time: SimTime,
    plan: CrimeDirectMemoryPlan,
    planned_memory_transaction: CrimeMemoryTransaction,
) -> Result<Mips4ExecutionCompletion, Ip32MachineDispatchError> {
    registry
        .get_resolved_mut(control.slots.crime)?
        .accept(CrimeSysAdRequest {
            time: delivery_time,
            transaction: cpu_transaction.clone(),
        })?;
    let memory_transaction = match registry.get_resolved_mut(control.slots.crime)?.poll()? {
        CrimePoll::Action(CrimeAction::StartMemory(transaction))
            if transaction == planned_memory_transaction =>
        {
            transaction
        }
        _ => {
            return Err(fast_transaction_invariant(
                "CRIME memory request changed after planning",
            ));
        }
    };
    let delivered = registry
        .get_resolved_mut(control.slots.memory)?
        .begin_direct_cpu_transaction(plan, memory_transaction);
    if matches!(delivered.transfer.view(), CrimeTransferView::Write { .. }) {
        invalidate_ram_code_sources(
            control,
            Some((delivered.address, delivered.transfer.length())),
        );
    }
    let sdram_completion = registry
        .get_resolved_mut(control.slots.sdram)?
        .accept(delivered);
    let (controller, memory_completion) = registry
        .get_resolved_mut(control.slots.memory)?
        .finish_direct_cpu_transaction(plan, sdram_completion);
    if controller != component_ids::CRIME {
        return Err(Ip32MachineDispatchError::UnexpectedController(controller));
    }
    registry
        .get_resolved_mut(control.slots.crime)?
        .complete(memory_completion);
    let completion = match registry.get_resolved_mut(control.slots.crime)?.poll()? {
        CrimePoll::Action(CrimeAction::CompleteSysAd(completion))
            if completion.id == cpu_transaction.id =>
        {
            completion
        }
        _ => {
            return Err(fast_transaction_invariant(
                "CRIME memory completion changed after planning",
            ));
        }
    };
    control.memory_transactions = control.memory_transactions.saturating_add(1);
    Ok(completion.payload)
}

#[cfg(feature = "jit")]
fn fast_transaction_invariant(message: &str) -> Ip32MachineDispatchError {
    Ip32MachineDispatchError::Cpu(R5000CpuError::Block(message.to_owned()))
}

#[cfg(feature = "jit")]
fn plan_fast_cpu_transaction<S>(
    registry: &ComponentRegistry,
    context: &Ip32DispatchContext<'_, '_, S>,
    control: &MachineControl,
    transaction: &ExecutionTransaction<Mips4ExecutionTransaction>,
    tracing_disabled: bool,
) -> Result<Option<Ip32CpuTransactionPlan>, Ip32MachineDispatchError>
where
    S: TraceSink,
{
    if context.stop_requested() || !tracing_disabled {
        return Ok(None);
    }
    let Some(sysad) = registry
        .get_resolved(control.slots.sysad)?
        .plan_direct_transaction()
    else {
        return Ok(None);
    };
    let delivery_time =
        context
            .now()
            .checked_add(sysad.request_delay())
            .ok_or(SchedulerError::TimeOverflow {
                time: context.now(),
                duration: sysad.request_delay(),
            })?;
    let route = match Crime::classify_sysad_route(transaction.payload) {
        CrimeSysAdRoute::SynchronousInternalRegister
            if registry
                .get_resolved(control.slots.crime)?
                .synchronous_sysad_read_ready(transaction.payload) =>
        {
            Ip32CpuTransactionRoute::InternalRegister
        }
        CrimeSysAdRoute::Memory => {
            let Some(memory_transaction) = registry
                .get_resolved(control.slots.crime)?
                .preview_synchronous_memory_request(transaction, delivery_time)
            else {
                return Ok(None);
            };
            let Some(plan) = registry
                .get_resolved(control.slots.memory)?
                .plan_direct_cpu_transaction(delivery_time)
            else {
                return Ok(None);
            };
            if !registry
                .get_resolved(control.slots.sdram)?
                .synchronous_transaction_ready(&memory_transaction)
            {
                return Ok(None);
            }
            Ip32CpuTransactionRoute::Memory {
                plan,
                transaction: memory_transaction,
            }
        }
        _ => return Ok(None),
    };
    let device_completion_time = match &route {
        Ip32CpuTransactionRoute::InternalRegister => delivery_time,
        Ip32CpuTransactionRoute::Memory { plan, .. } => delivery_time
            .checked_add(plan.service_delay())
            .ok_or(SchedulerError::TimeOverflow {
                time: delivery_time,
                duration: plan.service_delay(),
            })?
            .checked_add(plan.completion_delay())
            .ok_or(SchedulerError::TimeOverflow {
                time: delivery_time,
                duration: plan.completion_delay(),
            })?,
    };
    let completion_time = device_completion_time
        .checked_add(sysad.completion_delay())
        .ok_or(SchedulerError::TimeOverflow {
            time: device_completion_time,
            duration: sysad.completion_delay(),
        })?;
    if completion_time >= context.deadline()
        || context
            .next_event_time()
            .is_some_and(|event| completion_time >= event)
    {
        return Ok(None);
    }
    Ok(Some(Ip32CpuTransactionPlan {
        sysad,
        delivery_time,
        completion_time,
        route,
    }))
}

#[cfg(feature = "jit")]
fn fast_transaction_tracing_disabled<S>(context: &Ip32DispatchContext<'_, '_, S>) -> bool
where
    S: TraceSink,
{
    [
        TraceSource::Runtime,
        TraceSource::Scheduler,
        TraceSource::Component(component_ids::CPU_SYSAD_BUS),
        TraceSource::Component(component_ids::CRIME),
        TraceSource::Component(component_ids::CRIME_MEMORY_DOMAIN),
        TraceSource::Component(component_ids::RAM),
        TraceSource::Component(component_ids::CRIME_MACE_LINK),
        TraceSource::Component(component_ids::MACE),
        TraceSource::Component(component_ids::ISA_BUS),
    ]
    .into_iter()
    .all(|source| context.trace_interest(source) == TraceInterest::None)
}

fn drive_cpu<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
    mut action: ExecutionAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary>,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    let mut boundaries = 0;
    loop {
        match action {
            ExecutionAction::Transaction(transaction) => {
                return route_cpu_transaction(registry, context, control, transaction);
            }
            ExecutionAction::Boundary(_) => {
                boundaries += 1;
                let delay = control.cpu_clock.next_pclock_delay();
                if boundaries >= control.cpu_continuation_quantum {
                    schedule_cpu_step(context, control, delay)?;
                    return Ok(());
                }
                let next_time =
                    context
                        .now()
                        .checked_add(delay)
                        .ok_or(SchedulerError::TimeOverflow {
                            time: context.now(),
                            duration: delay,
                        })?;
                if !context.try_advance_to(next_time)? {
                    schedule_cpu_step(context, control, delay)?;
                    return Ok(());
                }
                action = registry.get_resolved_mut(control.slots.cpu)?.poll()?;
            }
            ExecutionAction::Idle => {
                #[cfg(test)]
                control.first_cpu_idle_time.get_or_insert(context.now());
                return Ok(());
            }
            ExecutionAction::Waiting { .. } => return Ok(()),
        }
    }
}

fn route_cpu_transaction<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
    transaction: ExecutionTransaction<Mips4ExecutionTransaction>,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    control.sysad_transactions = control.sysad_transactions.saturating_add(1);
    let disposition = registry
        .get_resolved_mut(control.slots.sysad)?
        .route(transaction);
    if let CrimeBusDisposition::QueuedAndNeedsService {
        delay,
        epoch: generation,
    } = disposition
    {
        context.schedule_after(
            delay,
            component_ids::CPU_SYSAD_BUS,
            Ip32Event::SysAdBus(super::bus::Ip32SysAdBusEvent::Service { generation }),
        )?;
    }
    Ok(())
}

fn schedule_cpu_step<S>(
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &MachineControl,
    delay: SimDuration,
) -> Result<(), EventChainError>
where
    S: TraceSink,
{
    context.schedule_after(
        delay,
        component_ids::CPU0,
        Ip32Event::CpuStep {
            generation: control.cpu_generation,
        },
    )?;
    Ok(())
}

fn drain_crime<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    loop {
        let poll = crime_with_trace_interest(registry, context, control.slots.crime)?.poll()?;
        let CrimePoll::Action(action) = poll else {
            return Ok(());
        };
        match action {
            CrimeAction::Schedule { delay, event } => {
                context.schedule_after(delay, component_ids::CRIME, Ip32Event::Crime(event))?;
            }
            CrimeAction::StartMemory(transaction) => {
                control.memory_transactions = control.memory_transactions.saturating_add(1);
                route_memory(registry, context, control.slots.memory, transaction)?;
            }
            CrimeAction::StartCmi(transaction) => {
                control.cmi_transactions = control.cmi_transactions.saturating_add(1);
                route_cmi(registry, context, control.slots.cmi, transaction)?;
            }
            CrimeAction::StartCgi(transaction) => {
                control.cgi_transactions = control.cgi_transactions.saturating_add(1);
                route_cgi(registry, context, control.slots.cgi, transaction)?;
            }
            CrimeAction::CompleteCmiDevice(completion) => {
                registry
                    .get_resolved_mut(control.slots.cmi)?
                    .accept_device_completion(completion);
                drain_cmi_bus(registry, context, control)?;
            }
            CrimeAction::CompleteCgiDevice(completion) => {
                registry
                    .get_resolved_mut(control.slots.cgi)?
                    .accept_device_completion(component_ids::CRIME, completion);
                drain_cgi_bus(registry, context, control)?;
            }
            CrimeAction::CompleteSysAd(completion) => {
                registry
                    .get_resolved_mut(control.slots.sysad)?
                    .accept_device_completion(completion);
                drain_sysad_bus(registry, context, control)?;
            }
            CrimeAction::SetIrq(transaction) => {
                context.request_barrier();
                registry
                    .get_typed_mut::<IrqBus>(component_ids::CPU_IRQ_BUS)?
                    .route(transaction)?;
                drain_irq_bus(registry)?;
            }
            CrimeAction::SignalCpu(CrimeCpuSignal::WarmReset) => {
                context.request_barrier();
                warm_reset(registry, context, control)?;
            }
            CrimeAction::SignalCpu(CrimeCpuSignal::HardReset) => {
                context.request_barrier();
                context.schedule_at(context.now(), component_ids::MACHINE, Ip32Event::HardReset)?;
            }
            CrimeAction::SignalMemory(signal) => {
                #[cfg(feature = "jit")]
                invalidate_ram_code_sources(control, None);
                registry
                    .get_typed_mut::<CrimeSdram>(component_ids::RAM)?
                    .accept(signal);
            }
            CrimeAction::Trace(event) => trace_crime(context, *event),
        }
    }
}

fn drain_irq_bus(registry: &mut ComponentRegistry) -> Result<(), Ip32MachineDispatchError> {
    loop {
        match registry
            .get_typed_mut::<IrqBus>(component_ids::CPU_IRQ_BUS)?
            .poll()
        {
            IrqBusAction::Deliver { target, delivery } => {
                if target != component_ids::CPU0 {
                    return Err(Ip32MachineDispatchError::UnexpectedController(target));
                }
                registry
                    .get_typed_mut::<R5000Cpu>(target)?
                    .accept(delivery)?;
            }
            IrqBusAction::Idle => return Ok(()),
        }
    }
}

fn drain_mace_irq_bus<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    loop {
        match registry
            .get_typed_mut::<IrqBus>(component_ids::MACE_IRQ_BUS)?
            .poll()
        {
            IrqBusAction::Deliver { target, delivery } => {
                if target != component_ids::MACE {
                    return Err(Ip32MachineDispatchError::UnexpectedController(target));
                }
                context.request_barrier();
                mace_with_trace_interest(registry, context, control.slots.mace)?
                    .accept(delivery)?;
                drain_mace(registry, context, control)?;
            }
            IrqBusAction::Idle => return Ok(()),
        }
    }
}

fn drain_mace<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    loop {
        let poll = mace_with_trace_interest(registry, context, control.slots.mace)?.poll()?;
        let MacePoll::Action(action) = poll else {
            return Ok(());
        };
        match action {
            MaceAction::Schedule { delay, event } => {
                context.schedule_after(delay, component_ids::MACE, Ip32Event::Mace(event))?;
            }
            MaceAction::StartCmi(transaction) => {
                control.cmi_transactions = control.cmi_transactions.saturating_add(1);
                route_cmi(registry, context, control.slots.cmi, transaction)?;
            }
            MaceAction::StartIsa(transaction) => {
                route_isa(registry, context, control, transaction)?
            }
            MaceAction::StartPci(transaction) => {
                context.request_barrier();
                let disposition = registry
                    .get_typed_mut::<PciBus>(component_ids::PCI_BUS)?
                    .route(transaction);
                if let se_device::bus::pci::PciBusDisposition::QueuedAndNeedsService { delay } =
                    disposition
                {
                    context.schedule_after(
                        delay,
                        component_ids::PCI_BUS,
                        Ip32Event::PciBusService,
                    )?;
                }
            }
            MaceAction::StartI2c(transaction) => {
                context.request_barrier();
                let index = if transaction.target == component_ids::VIDEO_INPUT0 {
                    0
                } else {
                    1
                };
                let bus_id = i2c_bus_id(index)?;
                let duration = I2cBus::duration(&transaction, IP32_TIMEBASE_HZ);
                if registry.get_typed_mut::<I2cBus>(bus_id)?.route(transaction) {
                    context.schedule_after(duration, bus_id, Ip32Event::I2cBusService { index })?;
                }
            }
            MaceAction::StartExternal(transaction) => {
                context.request_barrier();
                registry
                    .get_typed_mut::<MediaBus>(component_ids::MACE_MEDIA_BUS)?
                    .route(transaction);
                drain_media_bus(registry, context, control)?;
            }
            MaceAction::SetOneWire(drive) => {
                context.request_barrier();
                registry
                    .get_resolved_mut(control.slots.one_wire)?
                    .route(drive)?;
                drain_one_wire_bus(registry, context, control)?;
            }
            MaceAction::SetTwoWire { bus, drive } => {
                context.request_barrier();
                let index = ps2_bus_index(bus)?;
                registry
                    .get_resolved_mut(control.slots.ps2_buses[index])?
                    .route(drive)?;
                drain_ps2_bus(registry, context, control, index)?;
            }
            MaceAction::CompleteCmiDevice(completion) => {
                registry
                    .get_resolved_mut(control.slots.cmi)?
                    .accept_device_completion(completion);
                drain_cmi_bus(registry, context, control)?;
            }
            MaceAction::Trace(event) => trace_mace(context, *event),
        }
    }
}

fn ps2_bus_index(bus: ComponentId) -> Result<usize, Ip32MachineDispatchError> {
    if bus == component_ids::KEYBOARD_PS2_BUS {
        Ok(0)
    } else if bus == component_ids::MOUSE_PS2_BUS {
        Ok(1)
    } else {
        Err(Ip32MachineDispatchError::UnexpectedController(bus))
    }
}

fn drain_ps2_bus<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
    index: usize,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    loop {
        match registry
            .get_resolved_mut(control.slots.ps2_buses[index])?
            .poll()
        {
            TwoWireBusAction::Deliver { target, delivery } if target == component_ids::MACE => {
                mace_with_trace_interest(registry, context, control.slots.mace)?
                    .accept(delivery)?;
                drain_mace(registry, context, control)?;
            }
            TwoWireBusAction::Deliver { target, delivery }
                if target == component_ids::KEYBOARD && index == 0 =>
            {
                registry
                    .get_resolved_mut(control.slots.keyboard)?
                    .observe_lines(delivery)?;
                drain_keyboard(registry, context, control)?;
            }
            TwoWireBusAction::Deliver { target, delivery }
                if target == component_ids::MOUSE && index == 1 =>
            {
                registry
                    .get_resolved_mut(control.slots.mouse)?
                    .observe_lines(delivery)?;
                drain_mouse(registry, context, control)?;
            }
            TwoWireBusAction::Deliver { target, .. } => {
                return Err(Ip32MachineDispatchError::UnexpectedController(target));
            }
            TwoWireBusAction::Idle => return Ok(()),
        }
    }
}

fn drain_keyboard<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    loop {
        match registry.get_resolved_mut(control.slots.keyboard)?.poll() {
            Ps2KeyboardPoll::Action(Ps2KeyboardAction::Schedule { delay, event }) => {
                context.schedule_after(
                    delay,
                    component_ids::KEYBOARD,
                    Ip32Event::Keyboard(event),
                )?;
            }
            Ps2KeyboardPoll::Action(Ps2KeyboardAction::Drive(drive)) => {
                registry
                    .get_resolved_mut(control.slots.ps2_buses[0])?
                    .route(drive)?;
                drain_ps2_bus(registry, context, control, 0)?;
            }
            Ps2KeyboardPoll::Idle => return Ok(()),
        }
    }
}

fn drain_mouse<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    loop {
        match registry.get_resolved_mut(control.slots.mouse)?.poll() {
            Ps2MousePoll::Action(Ps2MouseAction::Schedule { delay, event }) => {
                context.schedule_after(delay, component_ids::MOUSE, Ip32Event::Mouse(event))?;
            }
            Ps2MousePoll::Action(Ps2MouseAction::Drive(drive)) => {
                registry
                    .get_resolved_mut(control.slots.ps2_buses[1])?
                    .route(drive)?;
                drain_ps2_bus(registry, context, control, 1)?;
            }
            Ps2MousePoll::Idle => return Ok(()),
        }
    }
}

fn drain_ds2502_actions<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    loop {
        match registry
            .get_resolved_mut(control.slots.nic_identity)?
            .poll()
        {
            Ds2502Action::Schedule { delay, event } => {
                context.schedule_after(
                    delay,
                    component_ids::NIC_IDENTITY,
                    Ip32Event::Ds2502(event),
                )?;
            }
            Ds2502Action::Drive(drive) => {
                registry
                    .get_resolved_mut(control.slots.one_wire)?
                    .route(drive)?;
            }
            Ds2502Action::Idle => return Ok(()),
        }
    }
}

fn drain_one_wire_bus<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    loop {
        match registry.get_resolved_mut(control.slots.one_wire)?.poll() {
            OneWireBusAction::Deliver { target, delivery } if target == component_ids::MACE => {
                mace_with_trace_interest(registry, context, control.slots.mace)?.accept(delivery);
            }
            OneWireBusAction::Deliver { target, delivery }
                if target == component_ids::NIC_IDENTITY =>
            {
                registry
                    .get_resolved_mut(control.slots.nic_identity)?
                    .accept(delivery);
                drain_ds2502_actions(registry, context, control)?;
            }
            OneWireBusAction::Deliver { target, .. } => {
                return Err(Ip32MachineDispatchError::UnexpectedController(target));
            }
            OneWireBusAction::Idle => return Ok(()),
        }
    }
}

fn route_isa<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &MachineControl,
    transaction: IsaTransaction,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    let disposition = registry
        .get_resolved_mut(control.slots.isa)?
        .route(transaction);
    if let IsaBusDisposition::QueuedAndNeedsService { delay, event } = disposition {
        context.schedule_after(delay, component_ids::ISA_BUS, Ip32Event::IsaBus(event))?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IsaPostDelivery {
    None,
    Rtc,
    Serial(ComponentId),
    Parallel,
}

fn isa_post_delivery(target: ComponentId) -> IsaPostDelivery {
    if target == component_ids::RTC {
        IsaPostDelivery::Rtc
    } else if matches!(target, component_ids::SERIAL0 | component_ids::SERIAL1) {
        IsaPostDelivery::Serial(target)
    } else if target == component_ids::PARALLEL_PORT {
        IsaPostDelivery::Parallel
    } else {
        IsaPostDelivery::None
    }
}

fn drain_isa_bus<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    loop {
        match registry.get_resolved_mut(control.slots.isa)?.poll() {
            IsaBusAction::Deliver {
                target,
                transaction,
            } => {
                let response = if target == component_ids::PROM {
                    registry
                        .get_resolved_mut(control.slots.prom)?
                        .accept(transaction)
                } else if target == component_ids::RTC {
                    let rtc = registry.get_resolved_mut(control.slots.rtc)?;
                    rtc.observe_time(context.now());
                    rtc.accept(transaction)
                } else if matches!(target, component_ids::SERIAL0 | component_ids::SERIAL1) {
                    let index = usize::from(target == component_ids::SERIAL1);
                    registry
                        .get_resolved_mut(control.slots.serial[index])?
                        .accept(transaction)
                } else if target == component_ids::PARALLEL_PORT {
                    registry
                        .get_resolved_mut(control.slots.parallel)?
                        .accept(transaction)
                } else {
                    IsaDeviceResponse::Complete(IsaCompletion {
                        id: transaction.id,
                        result: Err(se_device::bus::isa::IsaBusError::Address),
                    })
                };
                match response {
                    IsaDeviceResponse::Complete(completion) => {
                        registry
                            .get_resolved_mut(control.slots.isa)?
                            .accept_device_completion(completion);
                    }
                    IsaDeviceResponse::Deferred => context.request_barrier(),
                }
                match isa_post_delivery(target) {
                    IsaPostDelivery::None => {}
                    IsaPostDelivery::Rtc => drain_rtc(registry, context, control)?,
                    IsaPostDelivery::Serial(target) => {
                        drain_uart(registry, context, control, target)?
                    }
                    IsaPostDelivery::Parallel => drain_parallel(registry, context, control)?,
                }
            }
            IsaBusAction::Complete {
                controller,
                completion,
            } => {
                if controller != component_ids::MACE {
                    return Err(Ip32MachineDispatchError::UnexpectedController(controller));
                }
                mace_with_trace_interest(registry, context, control.slots.mace)?
                    .complete(completion);
                drain_mace(registry, context, control)?;
            }
            IsaBusAction::Schedule { delay, event } => {
                context.schedule_after(delay, component_ids::ISA_BUS, Ip32Event::IsaBus(event))?;
            }
            IsaBusAction::Idle => return Ok(()),
        }
    }
}

fn drain_rtc<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    loop {
        match registry.get_resolved_mut(control.slots.rtc)?.poll() {
            Ds1687Action::SetIrq(transaction) => {
                registry
                    .get_typed_mut::<IrqBus>(component_ids::MACE_IRQ_BUS)?
                    .route(transaction)?;
                drain_mace_irq_bus(registry, context, control)?;
            }
            Ds1687Action::Idle => return Ok(()),
        }
    }
}

fn drain_uart<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
    uart_id: ComponentId,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    loop {
        match registry
            .get_resolved_mut(control.slots.serial[serial_component_index(uart_id)])?
            .poll()
        {
            Uart16550Action::Schedule { delay, event } => {
                context.schedule_after(
                    delay,
                    uart_id,
                    Ip32Event::Uart {
                        port: serial_port_for_component(uart_id),
                        event,
                    },
                )?;
            }
            Uart16550Action::SetIrq(transaction) => {
                registry
                    .get_typed_mut::<IrqBus>(component_ids::MACE_IRQ_BUS)?
                    .route(transaction)?;
                drain_mace_irq_bus(registry, context, control)?;
            }
            Uart16550Action::Transmit { byte } => {
                registry
                    .get_typed_mut::<MediaBus>(component_ids::MACE_MEDIA_BUS)?
                    .route(MediaTransaction {
                        source: uart_id,
                        target: component_ids::MACHINE,
                        port: media_port_for_serial_component(uart_id),
                        payload: MediaPayload::Bytes(vec![byte]),
                    });
                drain_media_bus(registry, context, control)?;
            }
            Uart16550Action::Idle => return Ok(()),
        }
    }
}

fn drain_parallel<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    loop {
        match registry.get_resolved_mut(control.slots.parallel)?.poll() {
            Ieee1284Action::SetIrq(transaction) => {
                registry
                    .get_typed_mut::<IrqBus>(component_ids::MACE_IRQ_BUS)?
                    .route(transaction)?;
                drain_mace_irq_bus(registry, context, control)?;
            }
            Ieee1284Action::Idle => return Ok(()),
        }
    }
}

fn drain_pci_bus<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    loop {
        match registry
            .get_typed_mut::<PciBus>(component_ids::PCI_BUS)?
            .poll()
        {
            PciBusAction::Deliver { transaction, .. } => {
                let completion = if transaction.target == component_ids::SCSI_CONTROLLER {
                    registry
                        .get_typed_mut::<PciConfigurationEndpoint>(transaction.target)?
                        .accept(transaction)
                } else {
                    PciCompletion {
                        id: transaction.id,
                        status: PciStatus::MasterAbort,
                        data: vec![],
                    }
                };
                registry
                    .get_typed_mut::<PciBus>(component_ids::PCI_BUS)?
                    .complete(completion);
            }
            PciBusAction::Complete {
                controller,
                completion,
            } => {
                if controller != component_ids::MACE {
                    return Err(Ip32MachineDispatchError::UnexpectedController(controller));
                }
                mace_with_trace_interest(registry, context, control.slots.mace)?
                    .complete(completion);
                drain_mace(registry, context, control)?;
            }
            PciBusAction::Idle => return Ok(()),
        }
    }
}

fn drain_i2c_bus<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
    index: u8,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    let bus_id = i2c_bus_id(index)?;
    loop {
        match registry.get_typed_mut::<I2cBus>(bus_id)?.poll() {
            I2cBusAction::Deliver { transaction, .. } => {
                registry
                    .get_typed_mut::<I2cBus>(bus_id)?
                    .complete(I2cCompletion::Nack { id: transaction.id });
            }
            I2cBusAction::Complete {
                controller,
                completion,
            } => {
                if controller != component_ids::MACE {
                    return Err(Ip32MachineDispatchError::UnexpectedController(controller));
                }
                mace_with_trace_interest(registry, context, control.slots.mace)?
                    .complete(completion);
                drain_mace(registry, context, control)?;
            }
            I2cBusAction::Idle => return Ok(()),
        }
    }
}

fn drain_media_bus<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    loop {
        match registry
            .get_typed_mut::<MediaBus>(component_ids::MACE_MEDIA_BUS)?
            .poll()
        {
            MediaBusAction::Deliver {
                target,
                transaction,
            } => {
                if target == component_ids::MACE {
                    mace_with_trace_interest(registry, context, control.slots.mace)?
                        .accept_host_input(transaction)?;
                    drain_mace(registry, context, control)?;
                } else if matches!(target, component_ids::SERIAL0 | component_ids::SERIAL1) {
                    let MediaPayload::Bytes(bytes) = transaction.payload else {
                        return Err(Ip32MachineDispatchError::UnexpectedController(target));
                    };
                    registry
                        .get_resolved_mut(control.slots.serial[serial_component_index(target)])?
                        .receive_bytes(&bytes)?;
                    drain_uart(registry, context, control, target)?;
                } else {
                    enqueue_host_output(control, transaction, context.now());
                }
            }
            MediaBusAction::Idle => return Ok(()),
        }
    }
}

fn enqueue_host_output(control: &mut MachineControl, transaction: MediaTransaction, _now: SimTime) {
    let port = transaction.port;
    #[cfg(test)]
    if matches!(port, MediaPort::Serial0 | MediaPort::Serial1)
        && matches!(&transaction.payload, MediaPayload::Bytes(bytes) if !bytes.is_empty())
    {
        control.first_serial_output_time.get_or_insert(_now);
    }
    let index = media_port_index(port);
    let units = host_payload_units(&transaction.payload);
    let capacity = media_port_capacity(control.host_capacities, port);

    if units > capacity {
        control.host_dropped_output_bytes[index] =
            control.host_dropped_output_bytes[index].saturating_add(units as u64);
        return;
    }

    while control.host_output_units[index] + units > capacity {
        let Some(position) = control
            .host_outputs
            .iter()
            .position(|output| output.port == port)
        else {
            break;
        };
        let dropped = control
            .host_outputs
            .remove(position)
            .expect("located host output must remain present");
        let dropped_units = host_payload_units(&dropped.payload);
        control.host_output_units[index] =
            control.host_output_units[index].saturating_sub(dropped_units);
        control.host_dropped_output_bytes[index] =
            control.host_dropped_output_bytes[index].saturating_add(dropped_units as u64);
    }

    control.host_output_units[index] += units;
    control.host_outputs.push_back(Ip32HostOutput {
        port,
        payload: transaction.payload,
    });
}

fn i2c_bus_id(index: u8) -> Result<ComponentId, Ip32MachineDispatchError> {
    match index {
        0 => Ok(component_ids::I2C_BUS0),
        1 => Ok(component_ids::I2C_BUS1),
        _ => Err(Ip32MachineDispatchError::UnexpectedController(
            ComponentId::new(u64::from(index)),
        )),
    }
}

fn route_memory<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    slot: ComponentSlot<CrimeMemoryBus>,
    transaction: CrimeMemoryTransaction,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    let disposition = registry.get_resolved_mut(slot)?.route(transaction);
    if let CrimeBusDisposition::QueuedAndNeedsService { delay, epoch } = disposition {
        context.schedule_after(
            delay,
            component_ids::CRIME_MEMORY_DOMAIN,
            Ip32Event::CrimeMemoryBus(CrimeMemoryBusEvent::Service { epoch }),
        )?;
    }
    Ok(())
}

fn route_cmi<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    slot: ComponentSlot<CrimeCmiBus>,
    transaction: CrimeCmiTransaction,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    let disposition = registry.get_resolved_mut(slot)?.route(transaction);
    if let CrimeBusDisposition::QueuedAndNeedsService { delay, epoch } = disposition {
        context.schedule_after(
            delay,
            component_ids::CRIME_MACE_LINK,
            Ip32Event::CrimeCmiBus(CrimeCmiBusEvent::Service { epoch }),
        )?;
    }
    Ok(())
}

fn route_cgi<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    slot: ComponentSlot<CrimeCgiBus>,
    transaction: CrimeCgiTransaction,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    let disposition = registry.get_resolved_mut(slot)?.route(transaction);
    if let CrimeBusDisposition::QueuedAndNeedsService { delay, epoch } = disposition {
        context.schedule_after(
            delay,
            component_ids::CRIME_GBE_LINK,
            Ip32Event::CrimeCgiBus(CrimeCgiBusEvent::Service { epoch }),
        )?;
    }
    Ok(())
}

fn drain_sysad_bus<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    loop {
        let action = registry.get_resolved_mut(control.slots.sysad)?.poll();
        match action {
            Ip32SysAdBusAction::Deliver {
                target,
                transaction,
            } => {
                if target != component_ids::CRIME {
                    return Err(Ip32MachineDispatchError::UnexpectedController(target));
                }
                crime_with_trace_interest(registry, context, control.slots.crime)?.accept(
                    CrimeSysAdRequest {
                        time: context.now(),
                        transaction,
                    },
                )?;
                drain_crime(registry, context, control)?;
            }
            Ip32SysAdBusAction::Complete {
                controller,
                transaction,
                completion,
            } => {
                if controller != component_ids::CPU0 {
                    return Err(Ip32MachineDispatchError::UnexpectedController(controller));
                }
                trace_sysad_access(
                    registry,
                    context,
                    control.slots.cpu,
                    &transaction,
                    &completion,
                )?;
                registry
                    .get_resolved_mut(control.slots.cpu)?
                    .complete(completion);
                let same_time_event = context
                    .next_event_time()
                    .is_some_and(|time| time <= context.now());
                if !control.inline_sysad_completion || same_time_event {
                    schedule_cpu_step(context, control, SimDuration::ZERO)?;
                } else {
                    dispatch_cpu_step(registry, context, control)?;
                }
            }
            Ip32SysAdBusAction::Schedule { delay, event } => {
                context.schedule_after(
                    delay,
                    component_ids::CPU_SYSAD_BUS,
                    Ip32Event::SysAdBus(event),
                )?;
            }
            Ip32SysAdBusAction::Idle => return Ok(()),
        }
    }
}

fn drain_memory_bus<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    loop {
        let action = registry.get_resolved_mut(control.slots.memory)?.poll();
        match action {
            CrimeBusAction::Deliver {
                target,
                transaction,
            } => {
                if target != component_ids::RAM {
                    return Err(Ip32MachineDispatchError::UnexpectedController(target));
                }
                #[cfg(feature = "jit")]
                if matches!(transaction.transfer.view(), CrimeTransferView::Write { .. }) {
                    invalidate_ram_code_sources(
                        control,
                        Some((transaction.address, transaction.transfer.length())),
                    );
                }
                let completion = registry
                    .get_resolved_mut(control.slots.sdram)?
                    .accept(transaction);
                registry
                    .get_resolved_mut(control.slots.memory)?
                    .accept_device_completion(completion);
            }
            CrimeBusAction::Complete {
                controller,
                completion,
            } => {
                if controller != component_ids::CRIME {
                    return Err(Ip32MachineDispatchError::UnexpectedController(controller));
                }
                crime_with_trace_interest(registry, context, control.slots.crime)?
                    .complete(completion);
                drain_crime(registry, context, control)?;
            }
            CrimeBusAction::ScheduleService { delay } => {
                let event = registry
                    .get_resolved(control.slots.memory)?
                    .next_scheduled_event();
                context.schedule_after(
                    delay,
                    component_ids::CRIME_MEMORY_DOMAIN,
                    Ip32Event::CrimeMemoryBus(event),
                )?;
            }
            CrimeBusAction::Idle => return Ok(()),
        }
    }
}

fn drain_cmi_bus<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    loop {
        let action = registry.get_resolved_mut(control.slots.cmi)?.poll();
        match action {
            CrimeBusAction::Deliver {
                target,
                transaction,
            } => {
                let response = if target == component_ids::MACE {
                    let mace = mace_with_trace_interest(registry, context, control.slots.mace)?;
                    mace.observe_time(context.now());
                    mace.accept(transaction)
                } else if target == component_ids::CRIME {
                    let crime = crime_with_trace_interest(registry, context, control.slots.crime)?;
                    crime.observe_time(context.now());
                    crime.accept(transaction)
                } else {
                    return Err(Ip32MachineDispatchError::UnexpectedController(target));
                };
                match response {
                    CrimeLinkDeviceResponse::Complete(completion) => {
                        registry
                            .get_resolved_mut(control.slots.cmi)?
                            .accept_device_completion(completion);
                    }
                    CrimeLinkDeviceResponse::Deferred => context.request_barrier(),
                }
                drain_mace(registry, context, control)?;
                drain_crime(registry, context, control)?;
            }
            CrimeBusAction::Complete {
                controller,
                completion,
            } => {
                if controller == component_ids::CRIME {
                    crime_with_trace_interest(registry, context, control.slots.crime)?
                        .complete(completion);
                    drain_crime(registry, context, control)?;
                } else if controller == component_ids::MACE {
                    mace_with_trace_interest(registry, context, control.slots.mace)?
                        .complete(completion);
                    drain_mace(registry, context, control)?;
                } else {
                    return Err(Ip32MachineDispatchError::UnexpectedController(controller));
                }
            }
            CrimeBusAction::ScheduleService { delay } => {
                let event = registry
                    .get_resolved(control.slots.cmi)?
                    .next_scheduled_event();
                context.schedule_after(
                    delay,
                    component_ids::CRIME_MACE_LINK,
                    Ip32Event::CrimeCmiBus(event),
                )?;
            }
            CrimeBusAction::Idle => return Ok(()),
        }
    }
}

fn drain_cgi_bus<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    loop {
        let action = registry.get_resolved_mut(control.slots.cgi)?.poll();
        match action {
            CrimeBusAction::Deliver {
                target,
                transaction,
            } => {
                let response = if target == component_ids::GBE {
                    let gbe = registry.get_resolved_mut(control.slots.gbe)?;
                    gbe.observe_time(context.now());
                    gbe.accept(transaction)
                } else if target == component_ids::CRIME {
                    let crime = crime_with_trace_interest(registry, context, control.slots.crime)?;
                    crime.observe_time(context.now());
                    crime.accept(transaction)
                } else {
                    return Err(Ip32MachineDispatchError::UnexpectedController(target));
                };
                match response {
                    CrimeLinkDeviceResponse::Complete(completion) => {
                        registry
                            .get_resolved_mut(control.slots.cgi)?
                            .accept_device_completion(target, completion);
                    }
                    CrimeLinkDeviceResponse::Deferred => context.request_barrier(),
                }
                drain_crime(registry, context, control)?;
                drain_gbe(registry, context, control)?;
            }
            CrimeBusAction::Complete {
                controller,
                completion,
            } => {
                if controller == component_ids::CRIME {
                    crime_with_trace_interest(registry, context, control.slots.crime)?
                        .complete(completion);
                    drain_crime(registry, context, control)?;
                } else if controller == component_ids::GBE {
                    registry
                        .get_resolved_mut(control.slots.gbe)?
                        .complete(completion);
                    drain_gbe(registry, context, control)?;
                } else {
                    return Err(Ip32MachineDispatchError::UnexpectedController(controller));
                }
            }
            CrimeBusAction::ScheduleService { delay } => {
                let event = registry
                    .get_resolved_mut(control.slots.cgi)?
                    .next_scheduled_event()
                    .expect("every CGI scheduling action must carry one event");
                context.schedule_after(
                    delay,
                    component_ids::CRIME_GBE_LINK,
                    Ip32Event::CrimeCgiBus(event),
                )?;
            }
            CrimeBusAction::Idle => return Ok(()),
        }
    }
}

fn drain_gbe<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    loop {
        match registry.get_resolved_mut(control.slots.gbe)?.poll() {
            GbePoll::Action(GbeAction::Schedule { delay, event }) => {
                context.schedule_after(delay, component_ids::GBE, Ip32Event::Gbe(event))?;
            }
            GbePoll::Action(GbeAction::StartCgi(transaction)) => {
                control.cgi_transactions = control.cgi_transactions.saturating_add(1);
                route_cgi(registry, context, control.slots.cgi, transaction)?;
            }
            GbePoll::Action(GbeAction::SetDdc { bus, drive }) => {
                context.request_barrier();
                let index = if bus == component_ids::GBE_CRT_DDC_BUS {
                    0
                } else if bus == component_ids::GBE_FLAT_PANEL_DDC_BUS {
                    1
                } else {
                    return Err(Ip32MachineDispatchError::UnexpectedController(bus));
                };
                registry
                    .get_resolved_mut(control.slots.gbe_ddc[index])?
                    .route(drive)?;
                drain_gbe_ddc_bus(registry, control, index)?;
            }
            GbePoll::Action(GbeAction::CompleteCgiDevice(completion)) => {
                registry
                    .get_resolved_mut(control.slots.cgi)?
                    .accept_device_completion(component_ids::GBE, completion);
                drain_cgi_bus(registry, context, control)?;
            }
            GbePoll::Action(GbeAction::PublishFrame(frame)) => {
                if control.latest_display_frame.replace(frame).is_some() {
                    control.dropped_display_frames =
                        control.dropped_display_frames.saturating_add(1);
                }
            }
            GbePoll::Action(GbeAction::Trace(event)) => trace_gbe(context, *event),
            GbePoll::Idle => return Ok(()),
        }
    }
}

fn drain_gbe_ddc_bus(
    registry: &mut ComponentRegistry,
    control: &MachineControl,
    index: usize,
) -> Result<(), Ip32MachineDispatchError> {
    loop {
        match registry
            .get_resolved_mut(control.slots.gbe_ddc[index])?
            .poll()
        {
            TwoWireBusAction::Deliver { target, delivery } => {
                if target != component_ids::GBE {
                    return Err(Ip32MachineDispatchError::UnexpectedController(target));
                }
                registry
                    .get_resolved_mut(control.slots.gbe)?
                    .observe_ddc(delivery);
            }
            TwoWireBusAction::Idle => return Ok(()),
        }
    }
}

fn trace_sysad_access<S>(
    registry: &ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    cpu_slot: ComponentSlot<R5000Cpu>,
    transaction: &ExecutionTransaction<Mips4ExecutionTransaction>,
    completion: &ExecutionCompletion<Mips4ExecutionCompletion>,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    let source = TraceSource::Component(component_ids::CPU_SYSAD_BUS);
    if context.trace_interest(source) == TraceInterest::None {
        return Ok(());
    }
    let (address, width, operation) = match transaction.payload {
        Mips4ExecutionTransaction::Read {
            physical_address,
            size,
            ..
        } => (physical_address, size.bytes(), "read"),
        Mips4ExecutionTransaction::Write {
            physical_address,
            size,
            ..
        } => (physical_address, size.bytes(), "write"),
    };
    let cpu_pc = registry.get_resolved(cpu_slot)?.state().pc();
    let level = if matches!(completion.payload, Mips4ExecutionCompletion::BusError) {
        TraceLevel::Warn
    } else {
        TraceLevel::Trace
    };
    context.trace_lazy(source, level, "ip32.sysad", "access", || {
        [
            TraceField::u64("transaction_id", transaction.id.get() as u64),
            TraceField::hex64("physical_address", address),
            TraceField::u64("width", u64::from(width)),
            TraceField::string("operation", operation),
            TraceField::bool(
                "bus_error",
                matches!(completion.payload, Mips4ExecutionCompletion::BusError),
            ),
            TraceField::hex64("cpu_pc", cpu_pc),
        ]
    });
    Ok(())
}

fn trace_crime<S>(context: &mut Ip32DispatchContext<'_, '_, S>, event: CrimeTraceEvent)
where
    S: TraceSink,
{
    context.trace_lazy(
        TraceSource::Component(component_ids::CRIME),
        event.level,
        event.target,
        event.event,
        || {
            event
                .fields
                .iter()
                .map(|field| match field.value {
                    CrimeTraceValue::Bool(value) => TraceField::bool(field.key, value),
                    CrimeTraceValue::U64(value) => TraceField::u64(field.key, value),
                    CrimeTraceValue::Hex64(value) => TraceField::hex64(field.key, value),
                    CrimeTraceValue::String(value) => TraceField::string(field.key, value),
                })
                .collect::<Vec<_>>()
        },
    );
}

fn trace_mace<S>(context: &mut Ip32DispatchContext<'_, '_, S>, event: MaceTraceEvent)
where
    S: TraceSink,
{
    context.trace_lazy(
        TraceSource::Component(component_ids::MACE),
        event.level,
        event.target,
        event.event,
        || {
            event
                .fields
                .iter()
                .map(|field| match field.value {
                    MaceTraceValue::Bool(value) => TraceField::bool(field.key, value),
                    MaceTraceValue::U64(value) => TraceField::u64(field.key, value),
                    MaceTraceValue::Hex64(value) => TraceField::hex64(field.key, value),
                    MaceTraceValue::String(value) => TraceField::string(field.key, value),
                })
                .collect::<Vec<_>>()
        },
    );
}

fn trace_gbe<S>(context: &mut Ip32DispatchContext<'_, '_, S>, event: GbeTraceEvent)
where
    S: TraceSink,
{
    let level = match event.level {
        GbeTraceLevel::Error => TraceLevel::Error,
        GbeTraceLevel::Warn => TraceLevel::Warn,
        GbeTraceLevel::Info => TraceLevel::Info,
        GbeTraceLevel::Debug => TraceLevel::Debug,
        GbeTraceLevel::Trace => TraceLevel::Trace,
    };
    context.trace_lazy(
        TraceSource::Component(component_ids::GBE),
        level,
        &event.target,
        &event.event,
        || {
            event
                .fields
                .iter()
                .map(|field| match &field.value {
                    GbeTraceValue::Bool(value) => TraceField::bool(&field.key, *value),
                    GbeTraceValue::U64(value) => TraceField::u64(&field.key, *value),
                    GbeTraceValue::Hex64(value) => TraceField::hex64(&field.key, *value),
                    GbeTraceValue::String(value) => TraceField::string(&field.key, value),
                })
                .collect::<Vec<_>>()
        },
    );
}

#[cfg(test)]
mod tests;
