//! Runtime integration for the SGI O2 IP32 machine profile.

use core::fmt;

use se_core::role::{BusControllerRole, BusDeviceRole, BusRole};
use se_core::scheduler::{ScheduledEvent, ScheduledEventId, SchedulerError, SimDuration, SimTime};
use se_core::tracing::{NoopTraceSink, TraceField, TraceLevel, TraceSink, TraceSource};
use se_device::cpu::execution::protocol::{
    ExecutionAction, ExecutionCompletion, ExecutionTransaction, ExecutionTransactionId,
};
use se_device::cpu::mips4::config::{Mips4CacheConfig, Mips4Endianness};
use se_device::cpu::mips4::execution::bus::{Mips4ExecutionCompletion, Mips4ExecutionTransaction};
use se_device::cpu::mips4::model::r5000::boot_mode::R5000BootMode;
use se_device::cpu::mips4::model::r5000::cpu::{R5000Cpu, R5000CpuError};
use se_device::cpu::mips4::model::r5000::profile::R5000Profile;
use se_device::cpu::mips4::model::r5000::revision::R5000Revision;
use se_device::memory::{MemoryResponse, MemoryTransaction, Ram, Rom};
use se_runtime::registry::{ComponentRegistry, RegistryError, RegistryLookupError};
use se_runtime::runtime::{RunError, RunStatus, Runtime, RuntimeContext};

use super::address_map::{IP32_MAX_RAM_SIZE_BYTES, IP32_PROM_IMAGE_SIZE_BYTES, Ip32PhysicalRegion};
use super::bus::{Ip32BusRoute, Ip32CpuAddressBus, Ip32MmioStub, Ip32UnimplementedAccessPolicy};
use super::component_ids;
use super::event::Ip32Event;
use super::timing::IP32_TIMEBASE_HZ;

const DEFAULT_PROCESSOR_FREQUENCY_HZ: u64 = 180_000_000;
const DEFAULT_RAM_SIZE_BYTES: u64 = 64 * 1024 * 1024;
const PRIMARY_CACHE_SIZE_BYTES: u32 = 32 * 1024;
const SECONDARY_CACHE_SIZE_BYTES: u32 = 512 * 1024;
const CACHE_LINE_SIZE_BYTES: u32 = 32;
const SECONDARY_CACHE_ENABLE_BIT: u64 = 1 << 12;

/// Complete construction input for one IP32 machine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ip32MachineConfig {
    /// R5000 processor identity, byte order, clocks, and cache geometry.
    pub processor: R5000Profile,

    /// R5000 boot-mode serial stream sampled at reset.
    pub boot_mode: R5000BootMode,

    /// Installed RAM capacity in bytes.
    pub ram_size_bytes: u64,

    /// Exact 512 KiB System PROM image.
    pub prom_image: Vec<u8>,

    /// Behavior for mapped ASIC registers without implemented semantics.
    pub unimplemented_access_policy: Ip32UnimplementedAccessPolicy,
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
            ram_size_bytes: DEFAULT_RAM_SIZE_BYTES,
            prom_image: vec![0; IP32_PROM_IMAGE_SIZE_BYTES],
            unimplemented_access_policy: Ip32UnimplementedAccessPolicy::Strict,
        }
    }
}

/// Error returned while constructing an IP32 machine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Ip32MachineBuildError {
    /// Installed RAM is outside the supported range.
    InvalidRamSize {
        /// Requested capacity in bytes.
        size_bytes: u64,
    },

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
            Self::InvalidRamSize { size_bytes } => {
                write!(f, "invalid IP32 RAM size: {size_bytes} bytes")
            }
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

    /// A follow-up event could not be scheduled.
    Scheduler(SchedulerError),

    /// The reset generation counter was exhausted.
    GenerationOverflow,

    /// A zero-latency synchronous transaction remained incomplete.
    IncompleteSynchronousTransaction {
        /// Outstanding CPU transaction.
        transaction_id: ExecutionTransactionId,
    },
}

impl fmt::Display for Ip32MachineDispatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registry(error) => write!(f, "IP32 component lookup failed: {error}"),
            Self::Cpu(error) => write!(f, "IP32 CPU execution failed: {error}"),
            Self::Scheduler(error) => write!(f, "IP32 event scheduling failed: {error}"),
            Self::GenerationOverflow => write!(f, "IP32 reset generation overflow"),
            Self::IncompleteSynchronousTransaction { transaction_id } => {
                write!(
                    f,
                    "IP32 synchronous transaction {transaction_id} did not complete"
                )
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
    generation: u64,
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

        let mut runtime = Runtime::with_trace_sink(sink);
        let registry = runtime.registry_mut();
        insert_component(registry, Box::new(cpu))?;
        insert_component(
            registry,
            Box::new(Ip32CpuAddressBus::new(
                component_ids::CPU_SYSAD_BUS,
                "CPU SysAD bus",
                config.ram_size_bytes,
            )),
        )?;
        insert_component(
            registry,
            Box::new(Ram::new(
                component_ids::RAM,
                "System RAM",
                config.ram_size_bytes as usize,
            )),
        )?;
        insert_component(
            registry,
            Box::new(Rom::new(
                component_ids::PROM,
                "System PROM",
                config.prom_image,
            )),
        )?;
        for (id, name) in [
            (component_ids::CRIME, "CRIME"),
            (component_ids::MACE, "MACE"),
            (component_ids::GBE, "GBE"),
            (component_ids::VICE, "VICE"),
        ] {
            insert_component(
                registry,
                Box::new(Ip32MmioStub::new(
                    id,
                    name,
                    config.unimplemented_access_policy,
                )),
            )?;
        }

        Ok(Self {
            runtime,
            control: MachineControl {
                generation: 0,
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

    /// Schedules a reset event at the current simulated time.
    pub fn schedule_reset(&mut self) -> Result<ScheduledEventId, SchedulerError> {
        self.runtime
            .schedule_at(self.runtime.now(), component_ids::MACHINE, Ip32Event::Reset)
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
    if !(1..=IP32_MAX_RAM_SIZE_BYTES).contains(&config.ram_size_bytes) {
        return Err(Ip32MachineBuildError::InvalidRamSize {
            size_bytes: config.ram_size_bytes,
        });
    }
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
    component: Box<dyn se_core::component::Component>,
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
        Ip32Event::PowerOn | Ip32Event::Reset => {
            registry.reset_all();
            control.generation = control
                .generation
                .checked_add(1)
                .ok_or(Ip32MachineDispatchError::GenerationOverflow)?;
            control.cpu_clock.reset();
            context.schedule_at(
                context.now(),
                component_ids::CPU0,
                Ip32Event::CpuStep {
                    generation: control.generation,
                },
            )?;
        }
        Ip32Event::CpuStep { generation } if generation == control.generation => {
            dispatch_cpu_step(registry, context, control)?;
        }
        Ip32Event::CpuStep { .. } => {}
    }
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
    loop {
        let action = registry
            .get_typed_mut::<R5000Cpu>(component_ids::CPU0)?
            .poll()?;
        match action {
            ExecutionAction::Transaction(transaction) => {
                let completion = dispatch_cpu_transaction(registry, context, transaction)?;
                registry
                    .get_typed_mut::<R5000Cpu>(component_ids::CPU0)?
                    .complete(completion);
            }
            ExecutionAction::Boundary(_) => {
                context.schedule_after(
                    control.cpu_clock.next_pclock_delay(),
                    component_ids::CPU0,
                    Ip32Event::CpuStep {
                        generation: control.generation,
                    },
                )?;
                return Ok(());
            }
            ExecutionAction::Idle => return Ok(()),
            ExecutionAction::Waiting { transaction_id } => {
                return Err(Ip32MachineDispatchError::IncompleteSynchronousTransaction {
                    transaction_id,
                });
            }
        }
    }
}

fn dispatch_cpu_transaction<S>(
    registry: &mut ComponentRegistry,
    context: &mut RuntimeContext<'_, Ip32Event, S>,
    transaction: ExecutionTransaction<Mips4ExecutionTransaction>,
) -> Result<ExecutionCompletion<Mips4ExecutionCompletion>, Ip32MachineDispatchError>
where
    S: TraceSink,
{
    let route = registry
        .get_typed_mut::<Ip32CpuAddressBus>(component_ids::CPU_SYSAD_BUS)?
        .route(transaction);
    let (id, request, payload, target, region, no_ecc) = match route {
        Ip32BusRoute::Memory {
            region,
            target,
            offset,
            no_ecc,
            transaction,
        } => {
            let memory_transaction = to_memory_transaction(transaction.payload, offset);
            let response = if target == component_ids::RAM {
                registry
                    .get_typed_mut::<Ram>(target)?
                    .accept(memory_transaction)
            } else {
                registry
                    .get_typed_mut::<Rom>(target)?
                    .accept(memory_transaction)
            };
            (
                transaction.id,
                transaction.payload,
                from_memory_response(response),
                Some(target),
                Some(region),
                no_ecc,
            )
        }
        Ip32BusRoute::Stub {
            region,
            target,
            transaction,
        } => {
            let response = registry
                .get_typed_mut::<Ip32MmioStub>(target)?
                .accept(transaction.payload);
            (
                transaction.id,
                transaction.payload,
                response,
                Some(target),
                Some(region),
                false,
            )
        }
        Ip32BusRoute::Unmapped {
            region,
            transaction,
        } => (
            transaction.id,
            transaction.payload,
            Mips4ExecutionCompletion::BusError,
            None,
            region,
            false,
        ),
    };

    trace_bus_access(
        registry,
        context,
        CompletedBusAccess {
            transaction_id: id,
            request,
            completion: payload,
            target,
            region,
            no_ecc,
        },
    )?;
    Ok(ExecutionCompletion { id, payload })
}

const fn to_memory_transaction(
    transaction: Mips4ExecutionTransaction,
    offset: u64,
) -> MemoryTransaction {
    match transaction {
        Mips4ExecutionTransaction::Read { size, .. } => MemoryTransaction::Read {
            offset,
            size: size.bytes(),
        },
        Mips4ExecutionTransaction::Write {
            size,
            data,
            byte_enable,
            ..
        } => MemoryTransaction::Write {
            offset,
            size: size.bytes(),
            data,
            byte_enable,
        },
    }
}

const fn from_memory_response(response: MemoryResponse) -> Mips4ExecutionCompletion {
    match response {
        MemoryResponse::ReadData(data) => Mips4ExecutionCompletion::ReadData(data),
        MemoryResponse::WriteComplete => Mips4ExecutionCompletion::WriteComplete,
        MemoryResponse::AccessError => Mips4ExecutionCompletion::BusError,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CompletedBusAccess {
    transaction_id: ExecutionTransactionId,
    request: Mips4ExecutionTransaction,
    completion: Mips4ExecutionCompletion,
    target: Option<se_core::component::ComponentId>,
    region: Option<Ip32PhysicalRegion>,
    no_ecc: bool,
}

fn trace_bus_access<S>(
    registry: &ComponentRegistry,
    context: &mut RuntimeContext<'_, Ip32Event, S>,
    access: CompletedBusAccess,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    let cpu = registry.get_typed::<R5000Cpu>(component_ids::CPU0)?;
    let target_id = access
        .target
        .map_or(u64::MAX, se_core::component::ComponentId::get);
    let target_name = access
        .target
        .and_then(|id| registry.get(id))
        .map_or("unmapped", |component| component.name());
    let region_name = access
        .region
        .map_or("unmapped", Ip32PhysicalRegion::trace_name);
    let level = if matches!(access.completion, Mips4ExecutionCompletion::BusError) {
        TraceLevel::Warn
    } else {
        TraceLevel::Trace
    };
    let (physical_address, size, operation, request_data, byte_enable) = match access.request {
        Mips4ExecutionTransaction::Read {
            physical_address,
            size,
            ..
        } => (
            physical_address,
            size.bytes(),
            "read",
            0,
            ((1_u16 << size.bytes()) - 1) as u8,
        ),
        Mips4ExecutionTransaction::Write {
            physical_address,
            size,
            data,
            byte_enable,
            ..
        } => (physical_address, size.bytes(), "write", data, byte_enable),
    };
    let data = match (access.request, access.completion) {
        (_, Mips4ExecutionCompletion::ReadData(data)) => data,
        (Mips4ExecutionTransaction::Write { .. }, _) => request_data,
        (_, Mips4ExecutionCompletion::WriteComplete | Mips4ExecutionCompletion::BusError) => 0,
    };
    let fields = [
        TraceField::u64("transaction_id", access.transaction_id.get() as u64),
        TraceField::hex64("physical_address", physical_address),
        TraceField::u64("width", u64::from(size)),
        TraceField::string("operation", operation),
        TraceField::hex64("data", data),
        TraceField::hex64("byte_enable", u64::from(byte_enable)),
        TraceField::u64("target_component", target_id),
        TraceField::string("target_name", target_name),
        TraceField::string("region", region_name),
        TraceField::bool("no_ecc", access.no_ecc),
        TraceField::hex64("cpu_pc", cpu.state().pc()),
    ];
    context.trace(
        TraceSource::Component(component_ids::CPU_SYSAD_BUS),
        level,
        "ip32.sysad",
        "access",
        &fields,
    );
    Ok(())
}

#[cfg(test)]
mod tests;
