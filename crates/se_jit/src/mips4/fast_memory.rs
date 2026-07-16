//! Host-native MIPS IV fast-memory projections and timeline ABI.

use se_core::scheduler::FractionalClockProjection;
use se_device::cpu::mips4::execution::block::Mips4FastMemoryRuntime;

/// Affine side-effect-free register value available to native memory lowering.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4FastLinearReadProjection {
    /// Physical address of the register.
    pub physical_address: u64,

    /// Register value at `base_time_ticks`.
    pub base: u64,

    /// Simulated-time origin of `base`.
    pub base_time_ticks: u64,

    /// Frequency driving the affine register value.
    pub frequency_hz: u64,

    /// Simulated machine timebase frequency.
    pub timebase_hz: u64,
}

/// Reusable frequency constants for one native synchronous-memory timeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4FastMemoryParameters {
    cpu_timebase_hz: u64,
    cpu_frequency_hz: u64,
    cpu_base_ticks: u64,
    cpu_fraction_ticks: u64,
    cpu_frequency_reciprocal: u64,
    cpu_timebase_reciprocal: u64,
    bus_timebase_hz: u64,
    bus_frequency_hz: u64,
    bus_base_ticks: u64,
    bus_fraction_ticks: u64,
    bus_frequency_reciprocal: u64,
    cmi_timebase_hz: u64,
    cmi_frequency_hz: u64,
    cmi_base_ticks: u64,
    cmi_fraction_ticks: u64,
    cmi_frequency_reciprocal: u64,
    linear_read_timebase_hz: u64,
    linear_read_timebase_reciprocal: u64,
    secondary_linear_read_timebase_hz: u64,
    secondary_linear_read_timebase_reciprocal: u64,
}

impl Mips4FastMemoryParameters {
    /// Precomputes constants shared by every slice using the same clocks.
    pub fn new(
        cpu_clock: FractionalClockProjection,
        bus_clock: FractionalClockProjection,
        cmi_clock: FractionalClockProjection,
        linear_read_timebase_hz: u64,
        secondary_linear_read_timebase_hz: u64,
    ) -> Self {
        Self {
            cpu_timebase_hz: cpu_clock.timebase_hz(),
            cpu_frequency_hz: cpu_clock.frequency_hz(),
            cpu_base_ticks: cpu_clock.timebase_hz() / cpu_clock.frequency_hz(),
            cpu_fraction_ticks: cpu_clock.timebase_hz() % cpu_clock.frequency_hz(),
            cpu_frequency_reciprocal: division_reciprocal(cpu_clock.frequency_hz()),
            cpu_timebase_reciprocal: division_reciprocal(cpu_clock.timebase_hz()),
            bus_timebase_hz: bus_clock.timebase_hz(),
            bus_frequency_hz: bus_clock.frequency_hz(),
            bus_base_ticks: bus_clock.timebase_hz() / bus_clock.frequency_hz(),
            bus_fraction_ticks: bus_clock.timebase_hz() % bus_clock.frequency_hz(),
            bus_frequency_reciprocal: division_reciprocal(bus_clock.frequency_hz()),
            cmi_timebase_hz: cmi_clock.timebase_hz(),
            cmi_frequency_hz: cmi_clock.frequency_hz(),
            cmi_base_ticks: cmi_clock.timebase_hz() / cmi_clock.frequency_hz(),
            cmi_fraction_ticks: cmi_clock.timebase_hz() % cmi_clock.frequency_hz(),
            cmi_frequency_reciprocal: division_reciprocal(cmi_clock.frequency_hz()),
            linear_read_timebase_hz,
            linear_read_timebase_reciprocal: division_reciprocal(linear_read_timebase_hz),
            secondary_linear_read_timebase_hz,
            secondary_linear_read_timebase_reciprocal: division_reciprocal(
                secondary_linear_read_timebase_hz,
            ),
        }
    }

    /// Returns whether the constants describe the supplied projections.
    pub const fn matches(
        self,
        cpu_clock: FractionalClockProjection,
        bus_clock: FractionalClockProjection,
        cmi_clock: FractionalClockProjection,
        linear_read_timebase_hz: u64,
        secondary_linear_read_timebase_hz: u64,
    ) -> bool {
        self.cpu_timebase_hz == cpu_clock.timebase_hz()
            && self.cpu_frequency_hz == cpu_clock.frequency_hz()
            && self.bus_timebase_hz == bus_clock.timebase_hz()
            && self.bus_frequency_hz == bus_clock.frequency_hz()
            && self.cmi_timebase_hz == cmi_clock.timebase_hz()
            && self.cmi_frequency_hz == cmi_clock.frequency_hz()
            && self.linear_read_timebase_hz == linear_read_timebase_hz
            && self.secondary_linear_read_timebase_hz == secondary_linear_read_timebase_hz
    }
}

/// Stable ABI view used by native synchronous-memory timeline lowering.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct Mips4FastMemoryContext {
    native_linear_read_enabled: u64,
    start_time_ticks: u64,
    available_ticks: u64,
    cpu_timebase_hz: u64,
    cpu_frequency_hz: u64,
    cpu_remainder: u64,
    cpu_base_ticks: u64,
    cpu_fraction_ticks: u64,
    cpu_frequency_reciprocal: u64,
    cpu_timebase_reciprocal: u64,
    bus_timebase_hz: u64,
    bus_frequency_hz: u64,
    bus_remainder: u64,
    bus_base_ticks: u64,
    bus_fraction_ticks: u64,
    bus_frequency_reciprocal: u64,
    cmi_timebase_hz: u64,
    cmi_frequency_hz: u64,
    cmi_remainder: u64,
    cmi_base_ticks: u64,
    cmi_fraction_ticks: u64,
    cmi_frequency_reciprocal: u64,
    code_fetch_active: u64,
    code_fetch_shares_cmi: u64,
    code_fetch_fixed_ticks: u64,
    code_fetch_limit: u64,
    code_aux_timebase_hz: u64,
    code_aux_frequency_hz: u64,
    code_aux_remainder: u64,
    code_aux_base_ticks: u64,
    code_aux_fraction_ticks: u64,
    code_aux_frequency_reciprocal: u64,
    full_budget_admitted: u64,
    attempts: u64,
    completed: u64,
    cmi_completed: u64,
    last_transaction_fetch: u64,
    last_cmi_transaction_fetch: u64,
    last_delivery_ticks: u64,
    last_cmi_delivery_ticks: u64,
    linear_read_physical_address: u64,
    linear_read_base: u64,
    linear_read_base_time_ticks: u64,
    linear_read_frequency_hz: u64,
    linear_read_timebase_hz: u64,
    linear_read_timebase_reciprocal: u64,
    secondary_linear_read_physical_address: u64,
    secondary_linear_read_base: u64,
    secondary_linear_read_base_time_ticks: u64,
    secondary_linear_read_frequency_hz: u64,
    secondary_linear_read_timebase_hz: u64,
    secondary_linear_read_timebase_reciprocal: u64,
}

impl Mips4FastMemoryContext {
    /// Creates a bounded native timeline and validates overflow-free lowering.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        start_time_ticks: u64,
        available_ticks: u64,
        cpu_clock: FractionalClockProjection,
        bus_clock: FractionalClockProjection,
        cmi_clock: FractionalClockProjection,
        linear_read: Mips4FastLinearReadProjection,
        secondary_linear_read: Mips4FastLinearReadProjection,
        parameters: Mips4FastMemoryParameters,
        full_budget_admitted: bool,
    ) -> Self {
        let end_time = start_time_ticks.checked_add(available_ticks);
        let latest_linear_elapsed = end_time.map(|end| {
            end.saturating_sub(linear_read.base_time_ticks)
                .checked_mul(linear_read.frequency_hz)
        });
        let latest_secondary_linear_elapsed = end_time.map(|end| {
            end.saturating_sub(secondary_linear_read.base_time_ticks)
                .checked_mul(secondary_linear_read.frequency_hz)
        });
        let native_linear_read_enabled = u64::from(
            available_ticks != 0
                && cpu_clock.frequency_hz() > 1
                && cpu_clock.timebase_hz() > 1
                && bus_clock.frequency_hz() > 1
                && cmi_clock.frequency_hz() > 1
                && linear_read.timebase_hz > 1
                && secondary_linear_read.timebase_hz > 1
                && cpu_clock.elapsed(256).is_some()
                && bus_clock.elapsed(512).is_some()
                && cmi_clock.elapsed(512).is_some()
                && available_ticks
                    .checked_add(1)
                    .and_then(|ticks| ticks.checked_mul(cpu_clock.frequency_hz()))
                    .is_some()
                && latest_linear_elapsed.flatten().is_some()
                && latest_secondary_linear_elapsed.flatten().is_some()
                && linear_read.timebase_hz != 0
                && secondary_linear_read.timebase_hz != 0
                && parameters.matches(
                    cpu_clock,
                    bus_clock,
                    cmi_clock,
                    linear_read.timebase_hz,
                    secondary_linear_read.timebase_hz,
                ),
        );
        Self {
            native_linear_read_enabled,
            start_time_ticks,
            available_ticks,
            cpu_timebase_hz: cpu_clock.timebase_hz(),
            cpu_frequency_hz: cpu_clock.frequency_hz(),
            cpu_remainder: cpu_clock.remainder(),
            cpu_base_ticks: parameters.cpu_base_ticks,
            cpu_fraction_ticks: parameters.cpu_fraction_ticks,
            cpu_frequency_reciprocal: parameters.cpu_frequency_reciprocal,
            cpu_timebase_reciprocal: parameters.cpu_timebase_reciprocal,
            bus_timebase_hz: bus_clock.timebase_hz(),
            bus_frequency_hz: bus_clock.frequency_hz(),
            bus_remainder: bus_clock.remainder(),
            bus_base_ticks: parameters.bus_base_ticks,
            bus_fraction_ticks: parameters.bus_fraction_ticks,
            bus_frequency_reciprocal: parameters.bus_frequency_reciprocal,
            cmi_timebase_hz: cmi_clock.timebase_hz(),
            cmi_frequency_hz: cmi_clock.frequency_hz(),
            cmi_remainder: cmi_clock.remainder(),
            cmi_base_ticks: parameters.cmi_base_ticks,
            cmi_fraction_ticks: parameters.cmi_fraction_ticks,
            cmi_frequency_reciprocal: parameters.cmi_frequency_reciprocal,
            code_fetch_active: 0,
            code_fetch_shares_cmi: 0,
            code_fetch_fixed_ticks: 0,
            code_fetch_limit: 0,
            code_aux_timebase_hz: cmi_clock.timebase_hz(),
            code_aux_frequency_hz: cmi_clock.frequency_hz(),
            code_aux_remainder: cmi_clock.remainder(),
            code_aux_base_ticks: parameters.cmi_base_ticks,
            code_aux_fraction_ticks: parameters.cmi_fraction_ticks,
            code_aux_frequency_reciprocal: parameters.cmi_frequency_reciprocal,
            full_budget_admitted: u64::from(full_budget_admitted),
            attempts: 0,
            completed: 0,
            cmi_completed: 0,
            last_transaction_fetch: 0,
            last_cmi_transaction_fetch: 0,
            last_delivery_ticks: 0,
            last_cmi_delivery_ticks: 0,
            linear_read_physical_address: linear_read.physical_address,
            linear_read_base: linear_read.base,
            linear_read_base_time_ticks: linear_read.base_time_ticks,
            linear_read_frequency_hz: linear_read.frequency_hz,
            linear_read_timebase_hz: linear_read.timebase_hz,
            linear_read_timebase_reciprocal: parameters.linear_read_timebase_reciprocal,
            secondary_linear_read_physical_address: secondary_linear_read.physical_address,
            secondary_linear_read_base: secondary_linear_read.base,
            secondary_linear_read_base_time_ticks: secondary_linear_read.base_time_ticks,
            secondary_linear_read_frequency_hz: secondary_linear_read.frequency_hz,
            secondary_linear_read_timebase_hz: secondary_linear_read.timebase_hz,
            secondary_linear_read_timebase_reciprocal: parameters
                .secondary_linear_read_timebase_reciprocal,
        }
    }

    /// Returns the CPU clock projection captured at slice entry.
    pub const fn cpu_clock(&self) -> FractionalClockProjection {
        FractionalClockProjection::new(
            self.cpu_timebase_hz,
            self.cpu_frequency_hz,
            self.cpu_remainder,
        )
    }

    /// Returns the SysAD clock projection captured at slice entry.
    pub const fn bus_clock(&self) -> FractionalClockProjection {
        FractionalClockProjection::new(
            self.bus_timebase_hz,
            self.bus_frequency_hz,
            self.bus_remainder,
        )
    }

    /// Returns the CMI clock projection captured at slice entry.
    pub const fn cmi_clock(&self) -> FractionalClockProjection {
        FractionalClockProjection::new(
            self.cmi_timebase_hz,
            self.cmi_frequency_hz,
            self.cmi_remainder,
        )
    }

    /// Adds the stable code-source clock consumed before each guest operation.
    pub fn configure_code_fetch_timeline(
        &mut self,
        auxiliary_clock: FractionalClockProjection,
        fixed_ticks_per_fetch: u64,
        shares_cmi_clock: bool,
        fetch_limit: u64,
    ) -> bool {
        if self.code_fetch_active != 0
            || fetch_limit == 0
            || fetch_limit > 256
            || auxiliary_clock.frequency_hz() <= 1
            || auxiliary_clock.timebase_hz() <= 1
            || auxiliary_clock.elapsed(512).is_none()
            || (shares_cmi_clock && auxiliary_clock != self.cmi_clock())
        {
            return false;
        }
        self.code_fetch_active = 1;
        self.code_fetch_shares_cmi = u64::from(shares_cmi_clock);
        self.code_fetch_fixed_ticks = fixed_ticks_per_fetch;
        self.code_fetch_limit = fetch_limit;
        self.code_aux_timebase_hz = auxiliary_clock.timebase_hz();
        self.code_aux_frequency_hz = auxiliary_clock.frequency_hz();
        self.code_aux_remainder = auxiliary_clock.remainder();
        self.code_aux_base_ticks = auxiliary_clock.timebase_hz() / auxiliary_clock.frequency_hz();
        self.code_aux_fraction_ticks =
            auxiliary_clock.timebase_hz() % auxiliary_clock.frequency_hz();
        self.code_aux_frequency_reciprocal = division_reciprocal(auxiliary_clock.frequency_hz());
        true
    }

    /// Returns whether stable code fetches are folded into this timeline.
    pub const fn code_fetch_active(&self) -> bool {
        self.code_fetch_active != 0
    }

    /// Returns whether stable code fetches consume the same CMI clock.
    pub const fn code_fetch_shares_cmi(&self) -> bool {
        self.code_fetch_shares_cmi != 0
    }

    /// Returns the auxiliary stable-code clock projection.
    pub const fn code_aux_clock(&self) -> FractionalClockProjection {
        FractionalClockProjection::new(
            self.code_aux_timebase_hz,
            self.code_aux_frequency_hz,
            self.code_aux_remainder,
        )
    }

    /// Returns fixed simulated ticks consumed by every stable code fetch.
    pub const fn code_fetch_fixed_ticks(&self) -> u64 {
        self.code_fetch_fixed_ticks
    }

    /// Returns the maximum stable fetch prefix bound to this invocation.
    pub const fn code_fetch_limit(&self) -> u64 {
        self.code_fetch_limit
    }

    /// Returns the slice start time in simulated ticks.
    pub const fn start_time_ticks(&self) -> u64 {
        self.start_time_ticks
    }

    /// Returns the strict upper simulated-time bound for the slice.
    pub const fn available_ticks(&self) -> u64 {
        self.available_ticks
    }

    /// Returns whether slice planning proved the complete boundary budget fits.
    pub const fn full_budget_admitted(&self) -> bool {
        self.full_budget_admitted != 0
    }

    /// Tightens the strict simulated-time bound without extending it.
    pub fn limit_available_ticks(&mut self, available_ticks: u64) {
        if available_ticks < self.available_ticks {
            self.available_ticks = available_ticks;
            self.full_budget_admitted = 0;
        }
    }

    /// Records one attempted native or helper memory completion.
    pub fn record_attempt(&mut self) {
        self.attempts = self.attempts.saturating_add(1);
    }

    /// Records one completed memory transaction and its delivery offset.
    pub fn record_completion(&mut self, delivery_ticks: u64, fetches: u64) {
        self.completed = self.completed.saturating_add(1);
        self.last_delivery_ticks = delivery_ticks;
        self.last_transaction_fetch = fetches;
    }

    /// Returns attempted synchronous transactions.
    pub const fn attempts(&self) -> u64 {
        self.attempts
    }

    /// Returns completed synchronous transactions.
    pub const fn completed(&self) -> u64 {
        self.completed
    }

    /// Returns completed synchronous transactions routed over CMI.
    pub const fn cmi_completed(&self) -> u64 {
        self.cmi_completed
    }

    /// Returns the last completed request-delivery offset.
    pub const fn last_delivery_ticks(&self) -> u64 {
        self.last_delivery_ticks
    }

    /// Returns the last MACE delivery offset in simulation ticks.
    pub const fn last_cmi_delivery_ticks(&self) -> u64 {
        self.last_cmi_delivery_ticks
    }

    /// Returns the fetch index of the last completed synchronous transaction.
    pub const fn last_transaction_fetch(&self) -> u64 {
        self.last_transaction_fetch
    }

    /// Returns the fetch index of the last completed CMI transaction.
    pub const fn last_cmi_transaction_fetch(&self) -> u64 {
        self.last_cmi_transaction_fetch
    }
}

const fn division_reciprocal(divisor: u64) -> u64 {
    if divisor <= 1 {
        return 0;
    }
    ((u64::MAX as u128 + 1) / divisor as u128) as u64
}

/// Byte offsets of stable fields used by native fast-memory lowering.
pub const MIPS4_FAST_MEMORY_NATIVE_ENABLED_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, native_linear_read_enabled) as i32;
pub const MIPS4_FAST_MEMORY_START_TIME_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, start_time_ticks) as i32;
pub const MIPS4_FAST_MEMORY_AVAILABLE_TICKS_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, available_ticks) as i32;
pub const MIPS4_FAST_MEMORY_CPU_TIMEBASE_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, cpu_timebase_hz) as i32;
pub const MIPS4_FAST_MEMORY_CPU_FREQUENCY_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, cpu_frequency_hz) as i32;
pub const MIPS4_FAST_MEMORY_CPU_REMAINDER_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, cpu_remainder) as i32;
pub const MIPS4_FAST_MEMORY_CPU_BASE_TICKS_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, cpu_base_ticks) as i32;
pub const MIPS4_FAST_MEMORY_CPU_FRACTION_TICKS_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, cpu_fraction_ticks) as i32;
pub const MIPS4_FAST_MEMORY_CPU_FREQUENCY_RECIPROCAL_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, cpu_frequency_reciprocal) as i32;
pub const MIPS4_FAST_MEMORY_CPU_TIMEBASE_RECIPROCAL_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, cpu_timebase_reciprocal) as i32;
pub const MIPS4_FAST_MEMORY_BUS_TIMEBASE_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, bus_timebase_hz) as i32;
pub const MIPS4_FAST_MEMORY_BUS_FREQUENCY_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, bus_frequency_hz) as i32;
pub const MIPS4_FAST_MEMORY_BUS_REMAINDER_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, bus_remainder) as i32;
pub const MIPS4_FAST_MEMORY_BUS_BASE_TICKS_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, bus_base_ticks) as i32;
pub const MIPS4_FAST_MEMORY_BUS_FRACTION_TICKS_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, bus_fraction_ticks) as i32;
pub const MIPS4_FAST_MEMORY_BUS_FREQUENCY_RECIPROCAL_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, bus_frequency_reciprocal) as i32;
pub const MIPS4_FAST_MEMORY_CMI_FREQUENCY_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, cmi_frequency_hz) as i32;
pub const MIPS4_FAST_MEMORY_CMI_REMAINDER_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, cmi_remainder) as i32;
pub const MIPS4_FAST_MEMORY_CMI_BASE_TICKS_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, cmi_base_ticks) as i32;
pub const MIPS4_FAST_MEMORY_CMI_FRACTION_TICKS_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, cmi_fraction_ticks) as i32;
pub const MIPS4_FAST_MEMORY_CMI_FREQUENCY_RECIPROCAL_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, cmi_frequency_reciprocal) as i32;
pub const MIPS4_FAST_MEMORY_CODE_FETCH_ACTIVE_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, code_fetch_active) as i32;
pub const MIPS4_FAST_MEMORY_CODE_FETCH_SHARES_CMI_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, code_fetch_shares_cmi) as i32;
pub const MIPS4_FAST_MEMORY_CODE_FETCH_FIXED_TICKS_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, code_fetch_fixed_ticks) as i32;
pub const MIPS4_FAST_MEMORY_CODE_FETCH_LIMIT_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, code_fetch_limit) as i32;
pub const MIPS4_FAST_MEMORY_CODE_AUX_BASE_TICKS_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, code_aux_base_ticks) as i32;
pub const MIPS4_FAST_MEMORY_CODE_AUX_FRACTION_TICKS_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, code_aux_fraction_ticks) as i32;
pub const MIPS4_FAST_MEMORY_CODE_AUX_FREQUENCY_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, code_aux_frequency_hz) as i32;
pub const MIPS4_FAST_MEMORY_CODE_AUX_FREQUENCY_RECIPROCAL_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, code_aux_frequency_reciprocal) as i32;
pub const MIPS4_FAST_MEMORY_CODE_AUX_REMAINDER_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, code_aux_remainder) as i32;
pub const MIPS4_FAST_MEMORY_FULL_BUDGET_ADMITTED_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, full_budget_admitted) as i32;
pub const MIPS4_FAST_MEMORY_ATTEMPTS_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, attempts) as i32;
pub const MIPS4_FAST_MEMORY_COMPLETED_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, completed) as i32;
pub const MIPS4_FAST_MEMORY_CMI_COMPLETED_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, cmi_completed) as i32;
pub const MIPS4_FAST_MEMORY_LAST_TRANSACTION_FETCH_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, last_transaction_fetch) as i32;
pub const MIPS4_FAST_MEMORY_LAST_CMI_TRANSACTION_FETCH_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, last_cmi_transaction_fetch) as i32;
pub const MIPS4_FAST_MEMORY_LAST_DELIVERY_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, last_delivery_ticks) as i32;
pub const MIPS4_FAST_MEMORY_LAST_CMI_DELIVERY_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, last_cmi_delivery_ticks) as i32;
pub const MIPS4_FAST_MEMORY_LINEAR_ADDRESS_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, linear_read_physical_address) as i32;
pub const MIPS4_FAST_MEMORY_LINEAR_BASE_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, linear_read_base) as i32;
pub const MIPS4_FAST_MEMORY_LINEAR_BASE_TIME_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, linear_read_base_time_ticks) as i32;
pub const MIPS4_FAST_MEMORY_LINEAR_FREQUENCY_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, linear_read_frequency_hz) as i32;
pub const MIPS4_FAST_MEMORY_LINEAR_TIMEBASE_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, linear_read_timebase_hz) as i32;
pub const MIPS4_FAST_MEMORY_LINEAR_TIMEBASE_RECIPROCAL_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, linear_read_timebase_reciprocal) as i32;
pub const MIPS4_FAST_MEMORY_SECONDARY_LINEAR_ADDRESS_OFFSET: i32 = core::mem::offset_of!(
    Mips4FastMemoryContext,
    secondary_linear_read_physical_address
) as i32;
pub const MIPS4_FAST_MEMORY_SECONDARY_LINEAR_BASE_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, secondary_linear_read_base) as i32;
pub const MIPS4_FAST_MEMORY_SECONDARY_LINEAR_BASE_TIME_OFFSET: i32 = core::mem::offset_of!(
    Mips4FastMemoryContext,
    secondary_linear_read_base_time_ticks
) as i32;
pub const MIPS4_FAST_MEMORY_SECONDARY_LINEAR_FREQUENCY_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, secondary_linear_read_frequency_hz) as i32;
pub const MIPS4_FAST_MEMORY_SECONDARY_LINEAR_TIMEBASE_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, secondary_linear_read_timebase_hz) as i32;
pub const MIPS4_FAST_MEMORY_SECONDARY_LINEAR_TIMEBASE_RECIPROCAL_OFFSET: i32 = core::mem::offset_of!(
    Mips4FastMemoryContext,
    secondary_linear_read_timebase_reciprocal
) as i32;

/// Native extension to the portable fast-memory runtime.
pub trait Mips4NativeFastMemoryRuntime: Mips4FastMemoryRuntime {
    /// Returns the physical half-open range admitted by native read lowering.
    fn native_read_physical_range(&self) -> Option<(u64, u64)> {
        None
    }

    /// Returns the native timeline context for this slice.
    fn native_context(&mut self) -> Option<&mut Mips4FastMemoryContext> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::division_reciprocal;

    #[test]
    fn reciprocal_division_matches_unsigned_division() {
        fn divide(numerator: u64, divisor: u64) -> u64 {
            let reciprocal = division_reciprocal(divisor);
            let quotient = ((u128::from(numerator) * u128::from(reciprocal)) >> 64) as u64;
            let remainder = numerator - quotient * divisor;
            quotient + u64::from(remainder >= divisor)
        }

        let divisors = [
            2,
            3,
            7,
            9,
            10,
            66_000_000,
            100_000_000,
            180_000_000,
            1_000_000_000,
            u32::MAX as u64,
            u64::MAX,
        ];
        let mut random = 0x6a09_e667_f3bc_c909_u64;
        for divisor in divisors {
            for numerator in [
                0,
                1,
                divisor - 1,
                divisor,
                divisor.saturating_add(1),
                u64::MAX,
            ] {
                assert_eq!(divide(numerator, divisor), numerator / divisor);
            }
            for _ in 0..10_000 {
                random ^= random << 13;
                random ^= random >> 7;
                random ^= random << 17;
                assert_eq!(divide(random, divisor), random / divisor);
            }
        }
    }
}
