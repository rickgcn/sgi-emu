//! Runtime integration for the SGI O2 IP32 machine profile.

use core::fmt;

use se_core::component::{Component, ComponentId};
use se_core::role::{BusControllerRole, BusDeviceRole, BusRole};
use se_core::scheduler::{ScheduledEvent, ScheduledEventId, SchedulerError, SimDuration, SimTime};
use se_core::tracing::{NoopTraceSink, TraceField, TraceLevel, TraceSink, TraceSource};
use se_device::bus::i2c::{I2cBus, I2cBusAction, I2cCompletion};
use se_device::bus::irq::{
    IrqBus, IrqBusAction, IrqBusBuildError, IrqBusRouteError, IrqRoute, IrqSource, IrqTarget,
};
use se_device::bus::isa::{
    IsaBus, IsaBusAction, IsaBusDisposition, IsaCompletion, IsaDeviceResponse, IsaTransaction,
};
use se_device::bus::media::{MediaBus, MediaBusAction, MediaTransaction};
use se_device::bus::pci::{
    PciBus, PciBusAction, PciCompletion, PciConfigurationEndpoint, PciStatus,
};
use se_device::chipset::crime::config::CrimeConfig;
use se_device::chipset::crime::iou::{CrimeCgiBus, CrimeCmiBus};
use se_device::chipset::crime::memory::CrimeSdram;
use se_device::chipset::crime::memory::bus::CrimeMemoryBus;
use se_device::chipset::crime::protocol::{
    CRIME_IRQ_OUTPUT, CrimeAction, CrimeBusAction, CrimeBusDisposition, CrimeCgiTransaction,
    CrimeCmiTransaction, CrimeCpuSignal, CrimeLinkDeviceResponse, CrimeMemoryTransaction,
    CrimePoll, CrimeSysAdRequest, CrimeTraceEvent, CrimeTraceValue,
};
use se_device::chipset::crime::{Crime, CrimeError};
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
use se_device::cpu::mips4::execution::bus::{Mips4ExecutionCompletion, Mips4ExecutionTransaction};
use se_device::cpu::mips4::model::r5000::boot_mode::R5000BootMode;
use se_device::cpu::mips4::model::r5000::cpu::{
    R5000_IRQ_IP2, R5000Cpu, R5000CpuError, R5000CpuSignal, R5000IrqError,
};
use se_device::cpu::mips4::model::r5000::profile::R5000Profile;
use se_device::cpu::mips4::model::r5000::revision::R5000Revision;
use se_device::memory::flash::ReadArrayFlash;
use se_device::parallel::ieee1284::{IEEE1284_IRQ_OUTPUT, Ieee1284, Ieee1284Action};
use se_device::rtc::ds1687::{DS1687_IRQ_OUTPUT, Ds1687, Ds1687Action, Ds1687Config, Ds1687Error};
use se_device::serial::uart16550::{UART16550_IRQ_OUTPUT, Uart16550, Uart16550Action};
use se_runtime::registry::{ComponentRegistry, RegistryError, RegistryLookupError};
use se_runtime::runtime::{RunError, RunStatus, Runtime, RuntimeContext};

use super::address_map::IP32_PROM_IMAGE_SIZE_BYTES;
use super::bus::{Ip32GbeEndpoint, Ip32StubEndpoint, Ip32SysAdBus, Ip32SysAdBusAction};
use super::component_ids;
use super::event::{Ip32Event, Ip32HostInput, Ip32HostOutput};
use super::timing::IP32_TIMEBASE_HZ;

const DEFAULT_PROCESSOR_FREQUENCY_HZ: u64 = 180_000_000;
const PRIMARY_CACHE_SIZE_BYTES: u32 = 32 * 1024;
const SECONDARY_CACHE_SIZE_BYTES: u32 = 512 * 1024;
const CACHE_LINE_SIZE_BYTES: u32 = 32;
const SECONDARY_CACHE_ENABLE_BIT: u64 = 1 << 12;
const SDRAM_REFRESH_TICKS: u64 = 27_000;
const SDRAM_INITIALIZATION_TICKS: u64 = 120_000;
const ISA_CYCLE_TICKS: u64 = 1_000;
const PCI_CYCLE_TICKS: u64 = 30;

/// Complete construction input for one IP32 machine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ip32MachineConfig {
    /// R5000 processor identity, byte order, clocks, and cache geometry.
    pub processor: R5000Profile,

    /// R5000 boot-mode serial stream sampled at reset.
    pub boot_mode: R5000BootMode,

    /// CRIME chipset and physical SDRAM topology.
    pub crime: CrimeConfig,

    /// MACE I/O ASIC configuration.
    pub mace: MaceConfig,

    /// Deterministic RTC time and battery-backed NVRAM image.
    pub rtc: Ds1687Config,

    /// Exact 512 KiB System PROM image.
    pub prom_image: Vec<u8>,
}

impl Default for Ip32MachineConfig {
    fn default() -> Self {
        Self {
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
            prom_image: vec![0; IP32_PROM_IMAGE_SIZE_BYTES],
        }
    }
}

/// Error returned while constructing an IP32 machine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Ip32MachineBuildError {
    /// CRIME configuration is invalid.
    Crime(CrimeError),

    /// MACE configuration is invalid.
    Mace(MaceError),

    /// RTC configuration is invalid.
    Rtc(Ds1687Error),

    /// The CPU interrupt routing table is invalid.
    IrqBus(IrqBusBuildError),

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
}

impl fmt::Display for Ip32MachineBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Crime(error) => write!(f, "failed to construct CRIME: {error}"),
            Self::Mace(error) => write!(f, "failed to construct MACE: {error}"),
            Self::Rtc(error) => write!(f, "failed to construct DS1687: {error}"),
            Self::IrqBus(error) => write!(f, "failed to construct IP32 IRQ bus: {error}"),
            Self::InvalidPromSize { size_bytes } => {
                write!(f, "invalid IP32 PROM size: {size_bytes} bytes")
            }
            Self::InvalidProcessorFrequency { frequency_hz } => {
                write!(f, "invalid IP32 processor frequency: {frequency_hz} Hz")
            }
            Self::Cpu(error) => write!(f, "failed to construct IP32 CPU: {error}"),
            Self::Registry(error) => write!(f, "failed to register IP32 component: {error}"),
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

    /// The IRQ bus rejected a source transaction.
    IrqBus(IrqBusRouteError),

    /// The R5000 rejected an IRQ bus delivery.
    CpuIrq(R5000IrqError),

    /// A follow-up event could not be scheduled.
    Scheduler(SchedulerError),

    /// The reset generation counter was exhausted.
    GenerationOverflow,

    /// A completion named a controller not implemented by the current topology.
    UnexpectedController(ComponentId),
}

/// Error returned while scheduling deterministic host input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Ip32HostInputError {
    QueueFull(se_device::bus::media::MediaPort),
    Scheduler(SchedulerError),
}

impl fmt::Display for Ip32HostInputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
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
            Self::IrqBus(error) => write!(f, "IP32 IRQ routing failed: {error}"),
            Self::CpuIrq(error) => write!(f, "IP32 CPU IRQ delivery failed: {error}"),
            Self::Scheduler(error) => write!(f, "IP32 event scheduling failed: {error}"),
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

impl From<IrqBusRouteError> for Ip32MachineDispatchError {
    fn from(error: IrqBusRouteError) -> Self {
        Self::IrqBus(error)
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MachineControl {
    cpu_generation: u64,
    cpu_clock: CpuClock,
    host_capacities: MacePortConfig,
    host_reservations: [usize; 12],
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
                isa_bus: component_ids::ISA_BUS,
                prom: component_ids::PROM,
                rtc: component_ids::RTC,
                serial: [component_ids::SERIAL0, component_ids::SERIAL1],
                parallel: component_ids::PARALLEL_PORT,
                external_links: MaceExternalLinks {
                    i2c: [component_ids::VIDEO_INPUT0, component_ids::VIDEO_INPUT1],
                    audio: component_ids::AUDIO_SUBSYSTEM,
                    video_input_ab: component_ids::VIDEO_INPUT0,
                    video_input_cd: component_ids::VIDEO_INPUT1,
                    video_output: component_ids::VIDEO_OUTPUT,
                    ethernet: component_ids::ETHERNET_CONTROLLER,
                    keyboard: component_ids::KEYBOARD,
                    mouse: component_ids::MOUSE,
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
        insert_component(
            registry,
            Box::new(Ip32GbeEndpoint::new(
                component_ids::GBE,
                "GBE endpoint",
                config.crime.unimplemented_access_policy,
            )),
        )?;
        insert_component(
            registry,
            Box::new(Ip32StubEndpoint::new(component_ids::VICE, "VICE endpoint")),
        )?;
        insert_component(
            registry,
            Box::new(ReadArrayFlash::new(
                component_ids::PROM,
                "System flash",
                config.prom_image,
            )),
        )?;
        insert_component(registry, Box::new(rtc))?;
        insert_component(
            registry,
            Box::new(Uart16550::new(component_ids::SERIAL0, "Serial port 0")),
        )?;
        insert_component(
            registry,
            Box::new(Uart16550::new(component_ids::SERIAL1, "Serial port 1")),
        )?;
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

        Ok(Self {
            runtime,
            control: MachineControl {
                cpu_generation: 0,
                cpu_clock: CpuClock::new(processor_frequency_hz),
                host_capacities: config.mace.ports,
                host_reservations: [0; 12],
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
        let index = media_port_index(input.port);
        let queued = self
            .runtime
            .registry()
            .get_typed::<Mace>(component_ids::MACE)
            .expect("the IP32 MACE component must remain registered")
            .host_input_len(input.port);
        if queued + self.control.host_reservations[index]
            >= media_port_capacity(self.control.host_capacities, input.port)
        {
            return Err(Ip32HostInputError::QueueFull(input.port));
        }
        let id = self
            .runtime
            .schedule_at(at, component_ids::MACE, Ip32Event::HostInput(input))?;
        self.control.host_reservations[index] += 1;
        Ok(id)
    }

    /// Removes the oldest host-neutral output produced by MACE.
    pub fn poll_host_output(&mut self) -> Option<Ip32HostOutput> {
        self.runtime
            .registry_mut()
            .get_typed_mut::<Mace>(component_ids::MACE)
            .expect("the IP32 MACE component must remain registered")
            .poll_host_output()
            .map(|transaction| Ip32HostOutput {
                port: transaction.port,
                payload: transaction.payload,
            })
    }
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

fn dispatch_event<S>(
    event: ScheduledEvent<Ip32Event>,
    registry: &mut ComponentRegistry,
    context: &mut RuntimeContext<'_, Ip32Event, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    match event.payload {
        Ip32Event::PowerOn => power_on(registry, context, control)?,
        Ip32Event::HardReset => hard_reset(registry, context, control)?,
        Ip32Event::CpuStep { generation } if generation == control.cpu_generation => {
            dispatch_cpu_step(registry, context, control)?;
        }
        Ip32Event::CpuStep { .. } => {}
        Ip32Event::Crime(event) => {
            registry
                .get_typed_mut::<Crime>(component_ids::CRIME)?
                .handle_event(context.now(), event);
            drain_crime(registry, context, control)?;
        }
        Ip32Event::SysAdBus(event) => {
            registry
                .get_typed_mut::<Ip32SysAdBus>(component_ids::CPU_SYSAD_BUS)?
                .handle_event(event);
            drain_sysad_bus(registry, context, control)?;
        }
        Ip32Event::CrimeMemoryBus(event) => {
            registry
                .get_typed_mut::<CrimeMemoryBus>(component_ids::CRIME_MEMORY_DOMAIN)?
                .handle_event(event);
            drain_memory_bus(registry, context, control)?;
        }
        Ip32Event::CrimeCmiBus(event) => {
            registry
                .get_typed_mut::<CrimeCmiBus>(component_ids::CRIME_MACE_LINK)?
                .handle_event(event);
            drain_cmi_bus(registry, context, control)?;
        }
        Ip32Event::CrimeCgiBus(event) => {
            registry
                .get_typed_mut::<CrimeCgiBus>(component_ids::CRIME_GBE_LINK)?
                .handle_event(event);
            drain_cgi_bus(registry, context, control)?;
        }
        Ip32Event::Mace(event) => {
            registry
                .get_typed_mut::<Mace>(component_ids::MACE)?
                .handle_event(context.now(), event);
            drain_mace(registry, context, control)?;
        }
        Ip32Event::IsaBus(event) => {
            registry
                .get_typed_mut::<IsaBus>(component_ids::ISA_BUS)?
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
        Ip32Event::HostInput(input) => {
            control.host_reservations[media_port_index(input.port)] =
                control.host_reservations[media_port_index(input.port)].saturating_sub(1);
            let transaction = MediaTransaction {
                source: component_ids::MACE_MEDIA_BUS,
                target: component_ids::MACE,
                port: input.port,
                payload: input.payload,
            };
            registry
                .get_typed_mut::<Mace>(component_ids::MACE)?
                .accept_host_input(transaction)?;
            drain_mace(registry, context, control)?;
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
    context: &mut RuntimeContext<'_, Ip32Event, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    advance_cpu_generation(control)?;
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
    registry
        .get_typed_mut::<IsaBus>(component_ids::ISA_BUS)?
        .hard_reset();
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
    registry
        .get_typed_mut::<Mace>(component_ids::MACE)?
        .power_on(context.now());
    registry
        .get_typed_mut::<Ds1687>(component_ids::RTC)?
        .power_on(context.now());
    registry
        .get_typed_mut::<Uart16550>(component_ids::SERIAL0)?
        .reset();
    registry
        .get_typed_mut::<Uart16550>(component_ids::SERIAL1)?
        .reset();
    registry
        .get_typed_mut::<PciConfigurationEndpoint>(component_ids::SCSI_CONTROLLER)?
        .reset();
    registry
        .get_typed_mut::<Ieee1284>(component_ids::PARALLEL_PORT)?
        .reset();
    registry
        .get_typed_mut::<Ip32GbeEndpoint>(component_ids::GBE)?
        .reset();
    registry
        .get_typed_mut::<Ip32StubEndpoint>(component_ids::VICE)?
        .reset();
    registry
        .get_typed_mut::<CrimeMemoryBus>(component_ids::CRIME_MEMORY_DOMAIN)?
        .power_on(context.now());
    registry
        .get_typed_mut::<Crime>(component_ids::CRIME)?
        .power_on(context.now());
    drain_crime(registry, context, control)?;
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
    context: &mut RuntimeContext<'_, Ip32Event, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    advance_cpu_generation(control)?;
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
    registry
        .get_typed_mut::<IsaBus>(component_ids::ISA_BUS)?
        .hard_reset();
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
    registry
        .get_typed_mut::<Mace>(component_ids::MACE)?
        .hard_reset(context.now());
    registry
        .get_typed_mut::<Ds1687>(component_ids::RTC)?
        .hard_reset(context.now());
    registry
        .get_typed_mut::<Uart16550>(component_ids::SERIAL0)?
        .reset();
    registry
        .get_typed_mut::<Uart16550>(component_ids::SERIAL1)?
        .reset();
    registry
        .get_typed_mut::<PciConfigurationEndpoint>(component_ids::SCSI_CONTROLLER)?
        .reset();
    registry
        .get_typed_mut::<Ieee1284>(component_ids::PARALLEL_PORT)?
        .reset();
    registry
        .get_typed_mut::<Ip32GbeEndpoint>(component_ids::GBE)?
        .reset();
    registry
        .get_typed_mut::<Ip32StubEndpoint>(component_ids::VICE)?
        .reset();
    registry
        .get_typed_mut::<CrimeMemoryBus>(component_ids::CRIME_MEMORY_DOMAIN)?
        .hard_reset(context.now());
    registry
        .get_typed_mut::<Crime>(component_ids::CRIME)?
        .hard_reset(context.now());
    drain_crime(registry, context, control)?;
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
    context: &mut RuntimeContext<'_, Ip32Event, S>,
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
    registry
        .get_typed_mut::<Crime>(component_ids::CRIME)?
        .warm_reset();
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

fn dispatch_cpu_step<S>(
    registry: &mut ComponentRegistry,
    context: &mut RuntimeContext<'_, Ip32Event, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    let action = registry
        .get_typed_mut::<R5000Cpu>(component_ids::CPU0)?
        .poll()?;
    match action {
        ExecutionAction::Transaction(transaction) => {
            let disposition = registry
                .get_typed_mut::<Ip32SysAdBus>(component_ids::CPU_SYSAD_BUS)?
                .route(transaction);
            if let CrimeBusDisposition::QueuedAndNeedsService { delay } = disposition {
                let generation = registry
                    .get_typed::<Ip32SysAdBus>(component_ids::CPU_SYSAD_BUS)?
                    .generation();
                context.schedule_after(
                    delay,
                    component_ids::CPU_SYSAD_BUS,
                    Ip32Event::SysAdBus(super::bus::Ip32SysAdBusEvent::Service { generation }),
                )?;
            }
        }
        ExecutionAction::Boundary(_) => {
            context.schedule_after(
                control.cpu_clock.next_pclock_delay(),
                component_ids::CPU0,
                Ip32Event::CpuStep {
                    generation: control.cpu_generation,
                },
            )?;
        }
        ExecutionAction::Idle | ExecutionAction::Waiting { .. } => {}
    }
    Ok(())
}

fn drain_crime<S>(
    registry: &mut ComponentRegistry,
    context: &mut RuntimeContext<'_, Ip32Event, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    loop {
        let poll = registry
            .get_typed_mut::<Crime>(component_ids::CRIME)?
            .poll()?;
        let CrimePoll::Action(action) = poll else {
            return Ok(());
        };
        match action {
            CrimeAction::Schedule { delay, event } => {
                context.schedule_after(delay, component_ids::CRIME, Ip32Event::Crime(event))?;
            }
            CrimeAction::StartMemory(transaction) => {
                route_memory(registry, context, transaction)?;
            }
            CrimeAction::StartCmi(transaction) => route_cmi(registry, context, transaction)?,
            CrimeAction::StartCgi(transaction) => route_cgi(registry, context, transaction)?,
            CrimeAction::CompleteCmiDevice(completion) => {
                registry
                    .get_typed_mut::<CrimeCmiBus>(component_ids::CRIME_MACE_LINK)?
                    .accept_device_completion(completion);
                drain_cmi_bus(registry, context, control)?;
            }
            CrimeAction::CompleteCgiDevice(completion) => {
                registry
                    .get_typed_mut::<CrimeCgiBus>(component_ids::CRIME_GBE_LINK)?
                    .accept_device_completion(completion);
                drain_cgi_bus(registry, context, control)?;
            }
            CrimeAction::CompleteSysAd(completion) => {
                registry
                    .get_typed_mut::<Ip32SysAdBus>(component_ids::CPU_SYSAD_BUS)?
                    .accept_device_completion(completion);
                drain_sysad_bus(registry, context, control)?;
            }
            CrimeAction::SetIrq(transaction) => {
                registry
                    .get_typed_mut::<IrqBus>(component_ids::CPU_IRQ_BUS)?
                    .route(transaction)?;
                drain_irq_bus(registry)?;
            }
            CrimeAction::SignalCpu(CrimeCpuSignal::WarmReset) => {
                warm_reset(registry, context, control)?;
            }
            CrimeAction::SignalCpu(CrimeCpuSignal::HardReset) => {
                context.schedule_at(context.now(), component_ids::MACHINE, Ip32Event::HardReset)?;
            }
            CrimeAction::SignalMemory(signal) => {
                registry
                    .get_typed_mut::<CrimeSdram>(component_ids::RAM)?
                    .accept(signal);
            }
            CrimeAction::Trace(event) => trace_crime(context, event),
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
    context: &mut RuntimeContext<'_, Ip32Event, S>,
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
                registry.get_typed_mut::<Mace>(target)?.accept(delivery)?;
                drain_mace(registry, context, control)?;
            }
            IrqBusAction::Idle => return Ok(()),
        }
    }
}

fn drain_mace<S>(
    registry: &mut ComponentRegistry,
    context: &mut RuntimeContext<'_, Ip32Event, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    loop {
        let poll = registry
            .get_typed_mut::<Mace>(component_ids::MACE)?
            .poll()?;
        let MacePoll::Action(action) = poll else {
            return Ok(());
        };
        match action {
            MaceAction::Schedule { delay, event } => {
                context.schedule_after(delay, component_ids::MACE, Ip32Event::Mace(event))?;
            }
            MaceAction::StartCmi(transaction) => route_cmi(registry, context, transaction)?,
            MaceAction::StartIsa(transaction) => route_isa(registry, context, transaction)?,
            MaceAction::StartPci(transaction) => {
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
                registry
                    .get_typed_mut::<MediaBus>(component_ids::MACE_MEDIA_BUS)?
                    .route(transaction);
                drain_media_bus(registry)?;
            }
            MaceAction::CompleteCmiDevice(completion) => {
                registry
                    .get_typed_mut::<CrimeCmiBus>(component_ids::CRIME_MACE_LINK)?
                    .accept_device_completion(completion);
                drain_cmi_bus(registry, context, control)?;
            }
            MaceAction::Trace(event) => trace_mace(context, event),
        }
    }
}

fn route_isa<S>(
    registry: &mut ComponentRegistry,
    context: &mut RuntimeContext<'_, Ip32Event, S>,
    transaction: IsaTransaction,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    let disposition = registry
        .get_typed_mut::<IsaBus>(component_ids::ISA_BUS)?
        .route(transaction);
    if let IsaBusDisposition::QueuedAndNeedsService { delay } = disposition {
        let event = registry
            .get_typed::<IsaBus>(component_ids::ISA_BUS)?
            .next_service_event();
        context.schedule_after(delay, component_ids::ISA_BUS, Ip32Event::IsaBus(event))?;
    }
    Ok(())
}

fn drain_isa_bus<S>(
    registry: &mut ComponentRegistry,
    context: &mut RuntimeContext<'_, Ip32Event, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    loop {
        match registry
            .get_typed_mut::<IsaBus>(component_ids::ISA_BUS)?
            .poll()
        {
            IsaBusAction::Deliver {
                target,
                transaction,
            } => {
                let response = if target == component_ids::PROM {
                    registry
                        .get_typed_mut::<ReadArrayFlash>(target)?
                        .accept(transaction)
                } else if target == component_ids::RTC {
                    let rtc = registry.get_typed_mut::<Ds1687>(target)?;
                    rtc.observe_time(context.now());
                    rtc.accept(transaction)
                } else if matches!(target, component_ids::SERIAL0 | component_ids::SERIAL1) {
                    registry
                        .get_typed_mut::<Uart16550>(target)?
                        .accept(transaction)
                } else if target == component_ids::PARALLEL_PORT {
                    registry
                        .get_typed_mut::<Ieee1284>(target)?
                        .accept(transaction)
                } else {
                    IsaDeviceResponse::Complete(IsaCompletion {
                        id: transaction.id,
                        result: Err(se_device::bus::isa::IsaBusError::Address),
                    })
                };
                if let IsaDeviceResponse::Complete(completion) = response {
                    registry
                        .get_typed_mut::<IsaBus>(component_ids::ISA_BUS)?
                        .accept_device_completion(completion);
                }
                drain_rtc(registry, context, control)?;
                drain_uart(registry, context, control, component_ids::SERIAL0)?;
                drain_uart(registry, context, control, component_ids::SERIAL1)?;
                drain_parallel(registry, context, control)?;
            }
            IsaBusAction::Complete {
                controller,
                completion,
            } => {
                if controller != component_ids::MACE {
                    return Err(Ip32MachineDispatchError::UnexpectedController(controller));
                }
                registry
                    .get_typed_mut::<Mace>(controller)?
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
    context: &mut RuntimeContext<'_, Ip32Event, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    loop {
        match registry.get_typed_mut::<Ds1687>(component_ids::RTC)?.poll() {
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
    context: &mut RuntimeContext<'_, Ip32Event, S>,
    control: &mut MachineControl,
    uart_id: ComponentId,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    loop {
        match registry.get_typed_mut::<Uart16550>(uart_id)?.poll() {
            Uart16550Action::SetIrq(transaction) => {
                registry
                    .get_typed_mut::<IrqBus>(component_ids::MACE_IRQ_BUS)?
                    .route(transaction)?;
                drain_mace_irq_bus(registry, context, control)?;
            }
            Uart16550Action::Idle => return Ok(()),
        }
    }
}

fn drain_parallel<S>(
    registry: &mut ComponentRegistry,
    context: &mut RuntimeContext<'_, Ip32Event, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    loop {
        match registry
            .get_typed_mut::<Ieee1284>(component_ids::PARALLEL_PORT)?
            .poll()
        {
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
    context: &mut RuntimeContext<'_, Ip32Event, S>,
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
                registry
                    .get_typed_mut::<Mace>(controller)?
                    .complete(completion);
                drain_mace(registry, context, control)?;
            }
            PciBusAction::Idle => return Ok(()),
        }
    }
}

fn drain_i2c_bus<S>(
    registry: &mut ComponentRegistry,
    context: &mut RuntimeContext<'_, Ip32Event, S>,
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
                registry
                    .get_typed_mut::<Mace>(controller)?
                    .complete(completion);
                drain_mace(registry, context, control)?;
            }
            I2cBusAction::Idle => return Ok(()),
        }
    }
}

fn drain_media_bus(registry: &mut ComponentRegistry) -> Result<(), Ip32MachineDispatchError> {
    loop {
        match registry
            .get_typed_mut::<MediaBus>(component_ids::MACE_MEDIA_BUS)?
            .poll()
        {
            MediaBusAction::Deliver { transaction, .. } => {
                registry
                    .get_typed_mut::<Mace>(component_ids::MACE)?
                    .record_host_output(transaction)?;
            }
            MediaBusAction::Idle => return Ok(()),
        }
    }
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
    context: &mut RuntimeContext<'_, Ip32Event, S>,
    transaction: CrimeMemoryTransaction,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    let disposition = registry
        .get_typed_mut::<CrimeMemoryBus>(component_ids::CRIME_MEMORY_DOMAIN)?
        .route(transaction);
    if let CrimeBusDisposition::QueuedAndNeedsService { delay } = disposition {
        let event = registry
            .get_typed::<CrimeMemoryBus>(component_ids::CRIME_MEMORY_DOMAIN)?
            .next_scheduled_event();
        context.schedule_after(
            delay,
            component_ids::CRIME_MEMORY_DOMAIN,
            Ip32Event::CrimeMemoryBus(event),
        )?;
    }
    Ok(())
}

fn route_cmi<S>(
    registry: &mut ComponentRegistry,
    context: &mut RuntimeContext<'_, Ip32Event, S>,
    transaction: CrimeCmiTransaction,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    let disposition = registry
        .get_typed_mut::<CrimeCmiBus>(component_ids::CRIME_MACE_LINK)?
        .route(transaction);
    if let CrimeBusDisposition::QueuedAndNeedsService { delay } = disposition {
        let event = registry
            .get_typed::<CrimeCmiBus>(component_ids::CRIME_MACE_LINK)?
            .next_scheduled_event();
        context.schedule_after(
            delay,
            component_ids::CRIME_MACE_LINK,
            Ip32Event::CrimeCmiBus(event),
        )?;
    }
    Ok(())
}

fn route_cgi<S>(
    registry: &mut ComponentRegistry,
    context: &mut RuntimeContext<'_, Ip32Event, S>,
    transaction: CrimeCgiTransaction,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    let disposition = registry
        .get_typed_mut::<CrimeCgiBus>(component_ids::CRIME_GBE_LINK)?
        .route(transaction);
    if let CrimeBusDisposition::QueuedAndNeedsService { delay } = disposition {
        let event = registry
            .get_typed::<CrimeCgiBus>(component_ids::CRIME_GBE_LINK)?
            .next_scheduled_event();
        context.schedule_after(
            delay,
            component_ids::CRIME_GBE_LINK,
            Ip32Event::CrimeCgiBus(event),
        )?;
    }
    Ok(())
}

fn drain_sysad_bus<S>(
    registry: &mut ComponentRegistry,
    context: &mut RuntimeContext<'_, Ip32Event, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    loop {
        let action = registry
            .get_typed_mut::<Ip32SysAdBus>(component_ids::CPU_SYSAD_BUS)?
            .poll();
        match action {
            Ip32SysAdBusAction::Deliver {
                target,
                transaction,
            } => {
                if target != component_ids::CRIME {
                    return Err(Ip32MachineDispatchError::UnexpectedController(target));
                }
                registry
                    .get_typed_mut::<Crime>(target)?
                    .accept(CrimeSysAdRequest {
                        time: context.now(),
                        transaction,
                    })?;
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
                trace_sysad_access(registry, context, &transaction, &completion)?;
                registry
                    .get_typed_mut::<R5000Cpu>(controller)?
                    .complete(completion);
                context.schedule_at(
                    context.now(),
                    component_ids::CPU0,
                    Ip32Event::CpuStep {
                        generation: control.cpu_generation,
                    },
                )?;
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
    context: &mut RuntimeContext<'_, Ip32Event, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    loop {
        let action = registry
            .get_typed_mut::<CrimeMemoryBus>(component_ids::CRIME_MEMORY_DOMAIN)?
            .poll();
        match action {
            CrimeBusAction::Deliver {
                target,
                transaction,
            } => {
                if target != component_ids::RAM {
                    return Err(Ip32MachineDispatchError::UnexpectedController(target));
                }
                let completion = registry
                    .get_typed_mut::<CrimeSdram>(target)?
                    .accept(transaction);
                registry
                    .get_typed_mut::<CrimeMemoryBus>(component_ids::CRIME_MEMORY_DOMAIN)?
                    .accept_device_completion(completion);
            }
            CrimeBusAction::Complete {
                controller,
                completion,
            } => {
                if controller != component_ids::CRIME {
                    return Err(Ip32MachineDispatchError::UnexpectedController(controller));
                }
                registry
                    .get_typed_mut::<Crime>(controller)?
                    .complete(completion);
                drain_crime(registry, context, control)?;
            }
            CrimeBusAction::ScheduleService { delay } => {
                let event = registry
                    .get_typed::<CrimeMemoryBus>(component_ids::CRIME_MEMORY_DOMAIN)?
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
    context: &mut RuntimeContext<'_, Ip32Event, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    loop {
        let action = registry
            .get_typed_mut::<CrimeCmiBus>(component_ids::CRIME_MACE_LINK)?
            .poll();
        match action {
            CrimeBusAction::Deliver {
                target,
                transaction,
            } => {
                let response = if target == component_ids::MACE {
                    let mace = registry.get_typed_mut::<Mace>(target)?;
                    mace.observe_time(context.now());
                    mace.accept(transaction)
                } else if target == component_ids::CRIME {
                    let crime = registry.get_typed_mut::<Crime>(target)?;
                    crime.observe_time(context.now());
                    crime.accept(transaction)
                } else {
                    return Err(Ip32MachineDispatchError::UnexpectedController(target));
                };
                if let CrimeLinkDeviceResponse::Complete(completion) = response {
                    registry
                        .get_typed_mut::<CrimeCmiBus>(component_ids::CRIME_MACE_LINK)?
                        .accept_device_completion(completion);
                }
                drain_mace(registry, context, control)?;
                drain_crime(registry, context, control)?;
            }
            CrimeBusAction::Complete {
                controller,
                completion,
            } => {
                if controller == component_ids::CRIME {
                    registry
                        .get_typed_mut::<Crime>(controller)?
                        .complete(completion);
                    drain_crime(registry, context, control)?;
                } else if controller == component_ids::MACE {
                    registry
                        .get_typed_mut::<Mace>(controller)?
                        .complete(completion);
                    drain_mace(registry, context, control)?;
                } else {
                    return Err(Ip32MachineDispatchError::UnexpectedController(controller));
                }
            }
            CrimeBusAction::ScheduleService { delay } => {
                let event = registry
                    .get_typed::<CrimeCmiBus>(component_ids::CRIME_MACE_LINK)?
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
    context: &mut RuntimeContext<'_, Ip32Event, S>,
    control: &mut MachineControl,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    loop {
        let action = registry
            .get_typed_mut::<CrimeCgiBus>(component_ids::CRIME_GBE_LINK)?
            .poll();
        match action {
            CrimeBusAction::Deliver {
                target,
                transaction,
            } => {
                let response = if target == component_ids::GBE {
                    registry
                        .get_typed_mut::<Ip32GbeEndpoint>(target)?
                        .accept(transaction)
                } else if target == component_ids::CRIME {
                    let crime = registry.get_typed_mut::<Crime>(target)?;
                    crime.observe_time(context.now());
                    crime.accept(transaction)
                } else {
                    return Err(Ip32MachineDispatchError::UnexpectedController(target));
                };
                if let CrimeLinkDeviceResponse::Complete(completion) = response {
                    registry
                        .get_typed_mut::<CrimeCgiBus>(component_ids::CRIME_GBE_LINK)?
                        .accept_device_completion(completion);
                }
                drain_crime(registry, context, control)?;
            }
            CrimeBusAction::Complete {
                controller,
                completion,
            } => {
                if controller != component_ids::CRIME {
                    return Err(Ip32MachineDispatchError::UnexpectedController(controller));
                }
                registry
                    .get_typed_mut::<Crime>(controller)?
                    .complete(completion);
                drain_crime(registry, context, control)?;
            }
            CrimeBusAction::ScheduleService { delay } => {
                let event = registry
                    .get_typed::<CrimeCgiBus>(component_ids::CRIME_GBE_LINK)?
                    .next_scheduled_event();
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

fn trace_sysad_access<S>(
    registry: &ComponentRegistry,
    context: &mut RuntimeContext<'_, Ip32Event, S>,
    transaction: &ExecutionTransaction<Mips4ExecutionTransaction>,
    completion: &ExecutionCompletion<Mips4ExecutionCompletion>,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
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
    let cpu_pc = registry
        .get_typed::<R5000Cpu>(component_ids::CPU0)?
        .state()
        .pc();
    let fields = [
        TraceField::u64("transaction_id", transaction.id.get() as u64),
        TraceField::hex64("physical_address", address),
        TraceField::u64("width", u64::from(width)),
        TraceField::string("operation", operation),
        TraceField::bool(
            "bus_error",
            matches!(completion.payload, Mips4ExecutionCompletion::BusError),
        ),
        TraceField::hex64("cpu_pc", cpu_pc),
    ];
    let level = if matches!(completion.payload, Mips4ExecutionCompletion::BusError) {
        TraceLevel::Warn
    } else {
        TraceLevel::Trace
    };
    context.trace(
        TraceSource::Component(component_ids::CPU_SYSAD_BUS),
        level,
        "ip32.sysad",
        "access",
        &fields,
    );
    Ok(())
}

fn trace_crime<S>(context: &mut RuntimeContext<'_, Ip32Event, S>, event: CrimeTraceEvent)
where
    S: TraceSink,
{
    let fields = event
        .fields
        .iter()
        .map(|field| match field.value {
            CrimeTraceValue::Bool(value) => TraceField::bool(field.key, value),
            CrimeTraceValue::U64(value) => TraceField::u64(field.key, value),
            CrimeTraceValue::Hex64(value) => TraceField::hex64(field.key, value),
            CrimeTraceValue::String(value) => TraceField::string(field.key, value),
        })
        .collect::<Vec<_>>();
    context.trace(
        TraceSource::Component(component_ids::CRIME),
        event.level,
        event.target,
        event.event,
        &fields,
    );
}

fn trace_mace<S>(context: &mut RuntimeContext<'_, Ip32Event, S>, event: MaceTraceEvent)
where
    S: TraceSink,
{
    let fields = event
        .fields
        .iter()
        .map(|field| match field.value {
            MaceTraceValue::Bool(value) => TraceField::bool(field.key, value),
            MaceTraceValue::U64(value) => TraceField::u64(field.key, value),
            MaceTraceValue::Hex64(value) => TraceField::hex64(field.key, value),
            MaceTraceValue::String(value) => TraceField::string(field.key, value),
        })
        .collect::<Vec<_>>();
    context.trace(
        TraceSource::Component(component_ids::MACE),
        event.level,
        event.target,
        event.event,
        &fields,
    );
}

#[cfg(test)]
mod tests;
