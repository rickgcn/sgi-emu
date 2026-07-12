//! Runtime integration for the SGI O2 IP32 machine profile.

use core::fmt;

use se_core::component::{Component, ComponentId};
use se_core::role::{BusControllerRole, BusDeviceRole, BusRole};
use se_core::scheduler::{ScheduledEvent, ScheduledEventId, SchedulerError, SimDuration, SimTime};
use se_core::tracing::{NoopTraceSink, TraceField, TraceLevel, TraceSink, TraceSource};
use se_device::chipset::crime::config::CrimeConfig;
use se_device::chipset::crime::iou::{CrimeCgiBus, CrimeCmiBus};
use se_device::chipset::crime::memory::CrimeSdram;
use se_device::chipset::crime::memory::bus::CrimeMemoryBus;
use se_device::chipset::crime::protocol::{
    CrimeAction, CrimeBusAction, CrimeBusDisposition, CrimeCgiTransaction, CrimeCmiCompletion,
    CrimeCmiTransaction, CrimeCompletionPayload, CrimeCpuSignal, CrimeLinkDeviceResponse,
    CrimeMemoryTransaction, CrimePoll, CrimeSysAdRequest, CrimeTraceEvent, CrimeTraceValue,
    CrimeTransfer,
};
use se_device::chipset::crime::{Crime, CrimeError};
use se_device::cpu::execution::protocol::{
    ExecutionAction, ExecutionCompletion, ExecutionTransaction,
};
use se_device::cpu::mips4::config::{Mips4CacheConfig, Mips4Endianness};
use se_device::cpu::mips4::execution::bus::{Mips4ExecutionCompletion, Mips4ExecutionTransaction};
use se_device::cpu::mips4::model::r5000::boot_mode::R5000BootMode;
use se_device::cpu::mips4::model::r5000::cpu::{R5000Cpu, R5000CpuError, R5000CpuSignal};
use se_device::cpu::mips4::model::r5000::profile::R5000Profile;
use se_device::cpu::mips4::model::r5000::revision::R5000Revision;
use se_device::memory::{MemoryResponse, MemoryTransaction, Rom};
use se_runtime::registry::{ComponentRegistry, RegistryError, RegistryLookupError};
use se_runtime::runtime::{RunError, RunStatus, Runtime, RuntimeContext};

use super::address_map::IP32_PROM_IMAGE_SIZE_BYTES;
use super::bus::{
    Ip32GbeEndpoint, Ip32MaceDeviceResponse, Ip32MaceEndpoint, Ip32StubEndpoint, Ip32SysAdBus,
    Ip32SysAdBusAction,
};
use super::component_ids;
use super::event::Ip32Event;
use super::timing::IP32_TIMEBASE_HZ;

const DEFAULT_PROCESSOR_FREQUENCY_HZ: u64 = 180_000_000;
const PRIMARY_CACHE_SIZE_BYTES: u32 = 32 * 1024;
const SECONDARY_CACHE_SIZE_BYTES: u32 = 512 * 1024;
const CACHE_LINE_SIZE_BYTES: u32 = 32;
const SECONDARY_CACHE_ENABLE_BIT: u64 = 1 << 12;
const SDRAM_REFRESH_TICKS: u64 = 27_000;
const SDRAM_INITIALIZATION_TICKS: u64 = 120_000;

/// Complete construction input for one IP32 machine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ip32MachineConfig {
    /// R5000 processor identity, byte order, clocks, and cache geometry.
    pub processor: R5000Profile,

    /// R5000 boot-mode serial stream sampled at reset.
    pub boot_mode: R5000BootMode,

    /// CRIME chipset and physical SDRAM topology.
    pub crime: CrimeConfig,

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
            prom_image: vec![0; IP32_PROM_IMAGE_SIZE_BYTES],
        }
    }
}

/// Error returned while constructing an IP32 machine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Ip32MachineBuildError {
    /// CRIME configuration is invalid.
    Crime(CrimeError),

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

    /// A follow-up event could not be scheduled.
    Scheduler(SchedulerError),

    /// The reset generation counter was exhausted.
    GenerationOverflow,

    /// A completion named a controller not implemented by the current topology.
    UnexpectedController(ComponentId),
}

impl fmt::Display for Ip32MachineDispatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registry(error) => write!(f, "IP32 component lookup failed: {error}"),
            Self::Cpu(error) => write!(f, "IP32 CPU execution failed: {error}"),
            Self::Crime(error) => write!(f, "CRIME dispatch failed: {error}"),
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
            Box::new(Ip32MaceEndpoint::new(
                component_ids::MACE,
                "MACE endpoint",
                config.crime.unimplemented_access_policy,
            )),
        )?;
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
            Box::new(Rom::new(
                component_ids::PROM,
                "System PROM",
                config.prom_image,
            )),
        )?;

        Ok(Self {
            runtime,
            control: MachineControl {
                cpu_generation: 0,
                cpu_clock: CpuClock::new(processor_frequency_hz),
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
    }
    Ok(())
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
        .get_typed_mut::<Ip32SysAdBus>(component_ids::CPU_SYSAD_BUS)?
        .hard_reset();
    registry
        .get_typed_mut::<CrimeCmiBus>(component_ids::CRIME_MACE_LINK)?
        .hard_reset();
    registry
        .get_typed_mut::<CrimeCgiBus>(component_ids::CRIME_GBE_LINK)?
        .hard_reset();
    registry
        .get_typed_mut::<Ip32MaceEndpoint>(component_ids::MACE)?
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
        .get_typed_mut::<Ip32SysAdBus>(component_ids::CPU_SYSAD_BUS)?
        .hard_reset();
    registry
        .get_typed_mut::<CrimeCmiBus>(component_ids::CRIME_MACE_LINK)?
        .hard_reset();
    registry
        .get_typed_mut::<CrimeCgiBus>(component_ids::CRIME_GBE_LINK)?
        .hard_reset();
    registry
        .get_typed_mut::<Ip32MaceEndpoint>(component_ids::MACE)?
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
            CrimeAction::SignalCpu(CrimeCpuSignal::InterruptIp2(asserted)) => {
                registry
                    .get_typed_mut::<R5000Cpu>(component_ids::CPU0)?
                    .accept(R5000CpuSignal::ExternalInterrupts(u8::from(asserted)));
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
                    accept_mace(registry, transaction)?
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

fn accept_mace(
    registry: &mut ComponentRegistry,
    transaction: CrimeCmiTransaction,
) -> Result<CrimeLinkDeviceResponse<CrimeCmiCompletion>, Ip32MachineDispatchError> {
    match registry
        .get_typed_mut::<Ip32MaceEndpoint>(component_ids::MACE)?
        .accept(transaction)
    {
        Ip32MaceDeviceResponse::Complete(completion) => {
            Ok(CrimeLinkDeviceResponse::Complete(completion))
        }
        Ip32MaceDeviceResponse::Prom {
            id,
            offset,
            transfer,
        } => {
            let response = registry
                .get_typed_mut::<Rom>(component_ids::PROM)?
                .accept(to_memory_transaction(offset, &transfer));
            Ok(CrimeLinkDeviceResponse::Complete(CrimeCmiCompletion {
                id,
                result: from_memory_response(response, transfer.length()),
            }))
        }
    }
}

fn to_memory_transaction(offset: u64, transfer: &CrimeTransfer) -> MemoryTransaction {
    match transfer {
        CrimeTransfer::Read { length } => MemoryTransaction::Read {
            offset,
            size: *length as u8,
        },
        CrimeTransfer::Write { data, byte_enable } => {
            let mut lanes = [0; 8];
            let length = data.len().min(lanes.len());
            lanes[..length].copy_from_slice(&data[..length]);
            let enable = byte_enable
                .iter()
                .take(8)
                .enumerate()
                .fold(0_u8, |mask, (lane, enabled)| {
                    mask | (u8::from(*enabled) << lane)
                });
            MemoryTransaction::Write {
                offset,
                size: data.len() as u8,
                data: u64::from_le_bytes(lanes),
                byte_enable: enable,
            }
        }
    }
}

fn from_memory_response(
    response: MemoryResponse,
    length: usize,
) -> Result<CrimeCompletionPayload, se_device::chipset::crime::protocol::CrimeBusError> {
    match response {
        MemoryResponse::ReadData(data) if length <= 8 => Ok(CrimeCompletionPayload::ReadData(
            data.to_le_bytes()[..length].to_vec(),
        )),
        MemoryResponse::WriteComplete => Ok(CrimeCompletionPayload::WriteComplete),
        MemoryResponse::ReadData(_) | MemoryResponse::AccessError => {
            Err(se_device::chipset::crime::protocol::CrimeBusError::Access)
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

#[cfg(test)]
mod tests;
