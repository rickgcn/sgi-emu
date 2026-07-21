//! Functional R5000 CPU component and bus roles.

use core::fmt;

use se_core::component::{
    Component, ComponentId, ComponentStateError, validate_component_state_id,
};
use se_core::role::{BusControllerRole, BusDeviceRole};
use se_float::backend::FloatBackend;
use se_float::backend::softfloat3::mips4::Mips4SoftFloatBackend;

use crate::bus::irq::{IrqDelivery, IrqInput};
use crate::cpu::execution::functional::FunctionalExecutor;
use crate::cpu::execution::protocol::{
    ExecutionAction, ExecutionCompletion, ExecutionTransaction, ExecutionTransactionId,
    FunctionalExecutorError, FunctionalExecutorState,
};
use crate::cpu::execution::target::ExecutionTargetAction;
use crate::cpu::mips4::cp0::Mips4Cp0CacheErr;
use crate::cpu::mips4::execution::block::{
    Mips4BlockExit, Mips4BlockFrame, Mips4BlockKey, Mips4CodeSourceRequest, Mips4CodeWindow,
    Mips4FastMemoryRuntime,
};
use crate::cpu::mips4::execution::bus::{Mips4ExecutionCompletion, Mips4ExecutionTransaction};
use crate::cpu::mips4::execution::port::{
    Mips4BlockExecutionResult, Mips4BlockProbe, Mips4BlockSource, Mips4ExecutionPort,
    Mips4ReusableBlockExecution,
};
use crate::cpu::mips4::execution::state::{Mips4ExecutionConfigError, Mips4ExecutionState};
use crate::cpu::mips4::execution::target::{
    Mips4ExecutionBoundary, Mips4ExecutionSignal, Mips4ExecutionTarget, Mips4ExecutionTargetError,
    Mips4ExecutionTargetState,
};

use super::boot_mode::{R5000BootMode, R5000CountUpdateRate};
use super::execution_policy::R5000ExecutionPolicy;
use super::profile::R5000Profile;

/// External signal delivered to the R5000 through its bus-device role.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum R5000CpuSignal {
    /// Invalidates an outstanding load-linked reservation.
    InvalidateReservation,

    /// Raises a warm-reset exception and aborts any outstanding bus transaction.
    SoftReset,

    /// Latches a nonmaskable interrupt for the next instruction boundary.
    NonMaskableInterrupt,

    /// Raises a cache-error exception unless cache errors are disabled.
    CacheError(Mips4Cp0CacheErr),
}

/// Cumulative R5000 execution counters.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct R5000CpuStatistics {
    /// Instructions that retired normally.
    pub retired_instructions: u64,

    /// Architectural and error-level exception boundaries entered.
    pub exceptions: u64,

    /// Bus transactions published by the CPU.
    pub transactions: u64,
}

/// Observable disposition after one bounded R5000 execution slice.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum R5000ExecutionSliceAction {
    /// Architectural progress was made or the requested budget was zero.
    Progress,

    /// A typed runtime operation published a bus transaction.
    Transaction(ExecutionTransaction<Mips4ExecutionTransaction>),

    /// The processor is quiescent until an external interrupt or reset.
    Idle,

    /// A previously published transaction has not completed.
    Waiting {
        /// Outstanding transaction identifier.
        transaction_id: ExecutionTransactionId,
    },
}

/// Result of one bounded scalar or block execution slice.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct R5000ExecutionSlice {
    /// Instructions that retired normally.
    pub retired_instructions: u64,

    /// Total retirement and exception boundaries completed.
    pub boundaries: u64,

    /// Exception boundary that terminated the slice, when present.
    pub exception_boundary: Option<Mips4ExecutionBoundary>,

    /// Stable external instruction fetches completed by this slice.
    pub fast_fetches: u64,

    /// Simulated time consumed by stable external instruction fetches.
    pub simulated_time_ticks: u64,

    /// Dispatcher action required after the slice.
    pub action: R5000ExecutionSliceAction,
}

/// R5000 external hardware interrupt input IP2.
pub const R5000_IRQ_IP2: IrqInput = IrqInput::new(2);

/// R5000 external hardware interrupt input IP3.
pub const R5000_IRQ_IP3: IrqInput = IrqInput::new(3);

/// R5000 external hardware interrupt input IP4.
pub const R5000_IRQ_IP4: IrqInput = IrqInput::new(4);

/// R5000 external hardware interrupt input IP5.
pub const R5000_IRQ_IP5: IrqInput = IrqInput::new(5);

/// R5000 external hardware interrupt input IP6.
pub const R5000_IRQ_IP6: IrqInput = IrqInput::new(6);

/// R5000 interrupt delivery error.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum R5000IrqError {
    /// The delivery named an input that is not externally driven on this CPU.
    UnsupportedInput(IrqInput),
}

impl fmt::Display for R5000IrqError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedInput(input) => {
                write!(f, "unsupported R5000 IRQ input {}", input.get())
            }
        }
    }
}

impl std::error::Error for R5000IrqError {}

/// Terminal functional R5000 execution error.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum R5000CpuError {
    /// The requested architecture or cache configuration is invalid.
    Configuration(Mips4ExecutionConfigError),

    /// The generic executor or MIPS IV target rejected an operation.
    Execution(FunctionalExecutorError<Mips4ExecutionTargetError>),

    /// The translated-execution port failed or violated an invariant.
    Block(String),
}

impl fmt::Display for R5000CpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(error) => {
                write!(f, "invalid R5000 configuration: {error}")
            }
            Self::Execution(error) => write!(f, "R5000 execution failed: {error}"),
            Self::Block(error) => write!(f, "R5000 block execution failed: {error}"),
        }
    }
}

impl std::error::Error for R5000CpuError {}

#[derive(Default)]
struct R5000ReusableBlockFrame(Option<Mips4BlockFrame>);

impl Clone for R5000ReusableBlockFrame {
    fn clone(&self) -> Self {
        Self::default()
    }
}

fn reborrow_optional<'a, T: ?Sized>(value: &'a mut Option<&mut T>) -> Option<&'a mut T> {
    value.as_mut().map(|value| &mut **value)
}

/// Functional R5000 CPU with an injectable floating-point backend.
#[derive(Clone)]
pub struct R5000Cpu<F = Mips4SoftFloatBackend>
where
    F: FloatBackend,
{
    id: ComponentId,
    name: String,
    profile: R5000Profile,
    boot_mode: R5000BootMode,
    executor: FunctionalExecutor<Mips4ExecutionTarget<R5000ExecutionPolicy, F>>,
    half_pclock_remainder: u8,
    terminal_error: Option<R5000CpuError>,
    statistics: R5000CpuStatistics,
    reusable_block_frame: R5000ReusableBlockFrame,
}

/// Serializable dynamic state of an R5000 CPU.
#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct R5000CpuState {
    id: ComponentId,
    profile: R5000Profile,
    boot_mode: R5000BootMode,
    target: Mips4ExecutionTargetState,
    executor_state: FunctionalExecutorState,
    next_transaction_id: u128,
    queued_action: Option<ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary>>,
    half_pclock_remainder: u8,
    terminal_error: Option<R5000CpuError>,
    statistics: R5000CpuStatistics,
}

impl R5000Cpu<Mips4SoftFloatBackend> {
    /// Creates an R5000 using SoftFloat 3e as its reference FPU backend.
    pub fn new(
        id: ComponentId,
        name: impl Into<String>,
        profile: R5000Profile,
        boot_mode: R5000BootMode,
    ) -> Result<Self, R5000CpuError> {
        Self::with_float_backend(id, name, profile, boot_mode, Mips4SoftFloatBackend::new())
    }

    /// Captures the processor's architectural and executor state.
    pub fn save_state(&self) -> R5000CpuState {
        R5000CpuState {
            id: self.id,
            profile: self.profile,
            boot_mode: self.boot_mode,
            target: self.executor.target().save_state(),
            executor_state: self.executor.state(),
            next_transaction_id: self.executor.next_transaction_id(),
            queued_action: self.executor.queued_action().cloned(),
            half_pclock_remainder: self.half_pclock_remainder,
            terminal_error: self.terminal_error.clone(),
            statistics: self.statistics,
        }
    }

    /// Restores dynamic processor state while preserving the current policy and FPU backend.
    pub fn restore_state(&mut self, state: R5000CpuState) -> Result<(), ComponentStateError> {
        validate_component_state_id(self.id, state.id)?;
        if self.profile != state.profile {
            return Err(ComponentStateError::ConfigurationMismatch {
                component: self.id,
                field: "profile",
            });
        }
        if self.boot_mode != state.boot_mode {
            return Err(ComponentStateError::ConfigurationMismatch {
                component: self.id,
                field: "boot_mode",
            });
        }
        if let Err(invariant) = self.executor.target().validate_state(&state.target) {
            return Err(ComponentStateError::InvalidState {
                component: self.id,
                invariant,
            });
        }
        if state.half_pclock_remainder > 1 {
            return Err(ComponentStateError::InvalidState {
                component: self.id,
                invariant: "R5000 half-PClock remainder must be zero or one",
            });
        }
        let target_pending =
            Mips4ExecutionTarget::<R5000ExecutionPolicy, Mips4SoftFloatBackend>::state_has_pending(
                &state.target,
            );
        let queued_transaction = matches!(
            state.queued_action,
            Some(ExecutionTargetAction::Transaction(_))
        );
        let executor_valid = match state.executor_state {
            FunctionalExecutorState::Ready => target_pending == queued_transaction,
            FunctionalExecutorState::Waiting { transaction_id } => {
                state.queued_action.is_none()
                    && target_pending
                    && transaction_id.get() < state.next_transaction_id
            }
            FunctionalExecutorState::Failed => state.queued_action.is_none(),
        };
        if !executor_valid {
            return Err(ComponentStateError::InvalidState {
                component: self.id,
                invariant: "R5000 executor phase must match its pending target operation",
            });
        }

        self.executor.target_mut().restore_state(state.target);
        self.executor.restore_dynamic_state(
            state.executor_state,
            state.next_transaction_id,
            state.queued_action,
        );
        self.half_pclock_remainder = state.half_pclock_remainder;
        self.terminal_error = state.terminal_error;
        self.statistics = state.statistics;
        self.reusable_block_frame = R5000ReusableBlockFrame::default();
        Ok(())
    }
}

impl<F> R5000Cpu<F>
where
    F: FloatBackend,
{
    /// Creates an R5000 with a caller-supplied FPU backend.
    pub fn with_float_backend(
        id: ComponentId,
        name: impl Into<String>,
        profile: R5000Profile,
        boot_mode: R5000BootMode,
        float_backend: F,
    ) -> Result<Self, R5000CpuError> {
        let policy = R5000ExecutionPolicy::new(profile, boot_mode);
        policy.validate_cache_config().map_err(|error| {
            R5000CpuError::Configuration(Mips4ExecutionConfigError::Cache(error))
        })?;
        let target = Mips4ExecutionTarget::new(policy, float_backend)
            .map_err(R5000CpuError::Configuration)?;
        Ok(Self {
            id,
            name: name.into(),
            profile,
            boot_mode,
            executor: FunctionalExecutor::new(target),
            half_pclock_remainder: 0,
            terminal_error: None,
            statistics: R5000CpuStatistics::default(),
            reusable_block_frame: R5000ReusableBlockFrame::default(),
        })
    }

    /// Returns the configured processor profile.
    pub const fn profile(&self) -> R5000Profile {
        self.profile
    }

    /// Returns the sampled boot mode.
    pub const fn boot_mode(&self) -> R5000BootMode {
        self.boot_mode
    }

    /// Returns the complete architectural state.
    pub const fn state(&self) -> &Mips4ExecutionState {
        self.executor.target().state()
    }

    /// Returns cumulative execution counters.
    pub const fn statistics(&self) -> R5000CpuStatistics {
        self.statistics
    }

    /// Returns a stable external code-source request for the current PC.
    pub fn code_source_request(&self) -> Option<Mips4CodeSourceRequest> {
        self.executor.target().code_source_request()
    }

    /// Polls the next transaction, instruction boundary, idle state, or wait state.
    pub fn poll(
        &mut self,
    ) -> Result<ExecutionAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary>, R5000CpuError>
    {
        if let Some(error) = &self.terminal_error {
            return Err(error.clone());
        }
        if self.executor.ready_for_direct_execution()
            && self.executor.target().block_execution_ready()
        {
            self.reusable_block_frame.0 = None;
        }
        let action = self.executor.poll().map_err(R5000CpuError::Execution)?;
        match &action {
            ExecutionAction::Transaction(_) => {
                self.statistics.transactions = self.statistics.transactions.saturating_add(1);
            }
            ExecutionAction::Boundary(Mips4ExecutionBoundary::Retired { .. }) => {
                self.statistics.retired_instructions =
                    self.statistics.retired_instructions.saturating_add(1);
            }
            ExecutionAction::Boundary(
                Mips4ExecutionBoundary::Exception { .. }
                | Mips4ExecutionBoundary::ErrorException { .. },
            ) => {
                self.statistics.exceptions = self.statistics.exceptions.saturating_add(1);
            }
            ExecutionAction::Idle | ExecutionAction::Waiting { .. } => {}
        }
        if matches!(action, ExecutionAction::Boundary(_)) {
            if let ExecutionAction::Boundary(boundary) = &action
                && let Some(frame) = &mut self.reusable_block_frame.0
            {
                self.executor
                    .target()
                    .refresh_block_boundary(frame, boundary);
            }
            self.executor.target_mut().advance_random(1);
            self.advance_pclocks(1);
        }
        Ok(action)
    }

    /// Runs cached typed blocks up to a retirement budget.
    pub fn run_slice<P>(
        &mut self,
        port: &mut P,
        budget: u64,
    ) -> Result<R5000ExecutionSlice, R5000CpuError>
    where
        P: Mips4ExecutionPort,
        P::Error: fmt::Display,
    {
        let mut cached_retired = 0;
        let mut cached_exceptions = 0;
        let mut fast_memory = None;
        let result = self.run_slice_inner(
            port,
            budget,
            None,
            &mut fast_memory,
            &mut cached_retired,
            &mut cached_exceptions,
        );
        self.statistics.retired_instructions = self
            .statistics
            .retired_instructions
            .saturating_add(cached_retired);
        self.statistics.exceptions = self.statistics.exceptions.saturating_add(cached_exceptions);
        result
    }

    /// Runs queued protocol work or a reusable I-cache block before code-source probing.
    pub fn run_reusable_slice<P>(
        &mut self,
        port: &mut P,
        budget: u64,
    ) -> Result<Option<R5000ExecutionSlice>, R5000CpuError>
    where
        P: Mips4ExecutionPort,
        P::Error: fmt::Display,
    {
        let mut fast_memory = None;
        self.run_reusable_slice_inner(port, budget, &mut fast_memory)
    }

    fn run_reusable_slice_inner<P>(
        &mut self,
        port: &mut P,
        budget: u64,
        fast_memory: &mut Option<&mut P::FastMemoryRuntime>,
    ) -> Result<Option<R5000ExecutionSlice>, R5000CpuError>
    where
        P: Mips4ExecutionPort,
        P::Error: fmt::Display,
    {
        if let Some(error) = &self.terminal_error {
            return Err(error.clone());
        }
        if !self.executor.ready_for_direct_execution()
            || !self.executor.target().block_execution_ready()
        {
            let mut cached_retired = 0;
            let mut cached_exceptions = 0;
            let result = self.run_slice_inner(
                port,
                budget,
                None,
                fast_memory,
                &mut cached_retired,
                &mut cached_exceptions,
            );
            self.statistics.retired_instructions = self
                .statistics
                .retired_instructions
                .saturating_add(cached_retired);
            self.statistics.exceptions =
                self.statistics.exceptions.saturating_add(cached_exceptions);
            return result.map(Some);
        }
        let key = self.executor.target().block_key();
        if !matches!(
            port.probe(
                key,
                Mips4BlockSource::InstructionCache,
                self.executor.target(),
            ),
            Mips4BlockProbe::Ready { .. }
        ) {
            return Ok(None);
        }
        if self.executor.consume_ready_continuation()
            && !self.executor.target().dynamic_instruction_ready()
        {
            self.executor.target_mut().discard_dynamic_instruction();
        }
        let mut cached_retired = 0;
        let mut cached_exceptions = 0;
        let result = self.run_slice_inner(
            port,
            budget,
            Some(key),
            fast_memory,
            &mut cached_retired,
            &mut cached_exceptions,
        );
        self.statistics.retired_instructions = self
            .statistics
            .retired_instructions
            .saturating_add(cached_retired);
        self.statistics.exceptions = self.statistics.exceptions.saturating_add(cached_exceptions);
        result.map(Some)
    }

    /// Runs queued work or reusable blocks with a machine-proven fast-memory runtime.
    pub fn run_reusable_slice_with_fast_memory<P>(
        &mut self,
        port: &mut P,
        budget: u64,
        runtime: &mut P::FastMemoryRuntime,
    ) -> Result<Option<R5000ExecutionSlice>, R5000CpuError>
    where
        P: Mips4ExecutionPort,
        P::Error: fmt::Display,
    {
        let before = runtime.completed_transactions();
        let mut fast_memory = Some(&mut *runtime);
        let mut result = self.run_reusable_slice_inner(port, budget, &mut fast_memory);
        let completed = fast_memory
            .as_deref()
            .expect("fast-memory runtime remains installed for the slice")
            .completed_transactions()
            .checked_sub(before)
            .ok_or_else(|| self.block_error("fast-memory transaction counter moved backwards"))?;
        if completed != 0 {
            if let Ok(Some(R5000ExecutionSlice {
                action: R5000ExecutionSliceAction::Transaction(transaction),
                ..
            })) = &mut result
            {
                transaction.id = self
                    .executor
                    .account_transactions_before_waiting(completed)
                    .map_err(R5000CpuError::Execution)?;
            } else {
                self.executor
                    .account_ready_transactions(completed)
                    .map_err(R5000CpuError::Execution)?;
            }
            self.statistics.transactions = self.statistics.transactions.saturating_add(completed);
        }
        result
    }

    /// Runs a general typed slice with a machine-proven fast-memory runtime.
    pub fn run_slice_with_code_window_and_fast_memory<P>(
        &mut self,
        port: &mut P,
        budget: u64,
        code_window: Option<&Mips4CodeWindow>,
        runtime: &mut P::FastMemoryRuntime,
    ) -> Result<R5000ExecutionSlice, R5000CpuError>
    where
        P: Mips4ExecutionPort,
        P::Error: fmt::Display,
    {
        let before = runtime.completed_transactions();
        let mut fast_memory = Some(&mut *runtime);
        let mut result =
            self.run_slice_with_code_window_inner(port, budget, code_window, &mut fast_memory);
        let completed = fast_memory
            .as_deref()
            .expect("fast-memory runtime remains installed for the slice")
            .completed_transactions()
            .checked_sub(before)
            .ok_or_else(|| self.block_error("fast-memory transaction counter moved backwards"))?;
        if completed != 0 {
            if let Ok(R5000ExecutionSlice {
                action: R5000ExecutionSliceAction::Transaction(transaction),
                ..
            }) = &mut result
            {
                transaction.id = self
                    .executor
                    .account_transactions_before_waiting(completed)
                    .map_err(R5000CpuError::Execution)?;
            } else {
                self.executor
                    .account_ready_transactions(completed)
                    .map_err(R5000CpuError::Execution)?;
            }
            self.statistics.transactions = self.statistics.transactions.saturating_add(completed);
        }
        result
    }

    fn run_slice_inner<P>(
        &mut self,
        port: &mut P,
        budget: u64,
        initial_cached_block: Option<Mips4BlockKey>,
        fast_memory: &mut Option<&mut P::FastMemoryRuntime>,
        cached_retired: &mut u64,
        cached_exceptions: &mut u64,
    ) -> Result<R5000ExecutionSlice, R5000CpuError>
    where
        P: Mips4ExecutionPort,
        P::Error: fmt::Display,
    {
        if let Some(error) = &self.terminal_error {
            return Err(error.clone());
        }
        let mut accumulated = R5000ExecutionSlice {
            retired_instructions: 0,
            boundaries: 0,
            exception_boundary: None,
            fast_fetches: 0,
            simulated_time_ticks: 0,
            action: R5000ExecutionSliceAction::Progress,
        };
        if initial_cached_block.is_none()
            && (!self.executor.ready_for_direct_execution()
                || !self.executor.target().block_execution_ready())
        {
            return self.poll_protocol_slice();
        }
        let mut no_progress_retries = 0_u8;
        let mut frame = self.take_block_frame(budget.saturating_sub(accumulated.boundaries));
        let (mut block_key, mut check_execution_readiness) = match initial_cached_block {
            Some(key) => (key, false),
            None => {
                let key = self.executor.target().block_key_for_frame(&frame);
                (key, true)
            }
        };
        let mut deferred_boundaries = 0_u64;
        loop {
            let remaining = budget.saturating_sub(accumulated.boundaries);
            if remaining == 0 {
                self.advance_deferred_boundaries(&mut deferred_boundaries);
                self.commit_reusable_block_frame(frame);
                return Ok(accumulated);
            }
            if check_execution_readiness {
                if !self.executor.ready_for_direct_execution()
                    || !self.executor.target().block_execution_ready()
                {
                    self.advance_deferred_boundaries(&mut deferred_boundaries);
                    self.commit_reusable_block_frame(frame);
                    if accumulated.boundaries == 0 {
                        return self.poll_protocol_slice();
                    }
                    return Ok(accumulated);
                }
                if self.executor.consume_ready_continuation()
                    && !self.executor.target().dynamic_instruction_ready()
                {
                    self.executor.target_mut().discard_dynamic_instruction();
                }
                check_execution_readiness = false;
            }

            let previous_retired = frame.retired();
            let cached_execution = if block_key.code_guard == 0 {
                let result = {
                    let target = self.executor.target_mut();
                    port.execute_reusable(
                        block_key,
                        &mut frame,
                        target,
                        reborrow_optional(fast_memory),
                        deferred_boundaries != 0,
                    )
                };
                result.map_err(|error| self.block_error(error))?
            } else {
                Mips4ReusableBlockExecution::Missing
            };
            let (slice, persistent_frame, counter_barrier) = match cached_execution {
                Mips4ReusableBlockExecution::Executed(block_execution)
                    if matches!(
                        block_execution.exit,
                        Mips4BlockExit::BudgetExhausted
                            | Mips4BlockExit::Dispatch
                            | Mips4BlockExit::TimelineExhausted
                            | Mips4BlockExit::GuardInvalid
                    ) =>
                {
                    self.executor.target_mut().discard_dynamic_instruction();
                    let retired_instructions = frame
                        .retired()
                        .checked_sub(previous_retired)
                        .ok_or_else(|| {
                            self.block_error("block retirement counter moved backwards")
                        })?;
                    *cached_retired += retired_instructions;
                    accumulated.retired_instructions += retired_instructions;
                    accumulated.boundaries += retired_instructions;
                    deferred_boundaries += retired_instructions;

                    if block_execution.exit == Mips4BlockExit::TimelineExhausted {
                        self.advance_deferred_boundaries(&mut deferred_boundaries);
                        self.commit_reusable_block_frame(frame);
                        return Ok(accumulated);
                    }
                    if block_execution.counter_barrier {
                        self.advance_deferred_boundaries(&mut deferred_boundaries);
                        self.commit_reusable_block_frame(frame);
                        return Ok(accumulated);
                    }
                    if accumulated.boundaries >= budget || frame.budget() == 0 {
                        self.advance_deferred_boundaries(&mut deferred_boundaries);
                        self.commit_reusable_block_frame(frame);
                        return Ok(accumulated);
                    }
                    if retired_instructions == 0 {
                        no_progress_retries = no_progress_retries.saturating_add(1);
                        if no_progress_retries > 1 {
                            self.advance_deferred_boundaries(&mut deferred_boundaries);
                            return Err(self.block_error(format_args!(
                                "cached block dispatcher made no architectural progress with {remaining} boundaries requested, {} frame budget, {} frame retirements, and PC {:#018x}",
                                frame.budget(),
                                frame.retired(),
                                frame.pc(),
                            )));
                        }
                    } else {
                        no_progress_retries = 0;
                    }
                    block_key.pc = frame.pc();
                    block_key.next_pc = frame.next_pc();
                    block_key.delay_slot_branch_pc = frame.delay_slot_branch_pc();
                    continue;
                }
                Mips4ReusableBlockExecution::Executed(block_execution) => {
                    self.executor.target_mut().discard_dynamic_instruction();
                    let slice = self.finish_cached_block_exit(
                        &mut frame,
                        previous_retired,
                        block_execution,
                    )?;
                    *cached_retired += slice.retired_instructions;
                    *cached_exceptions += u64::from(slice.exception_boundary.is_some());
                    (slice, true, block_execution.counter_barrier)
                }
                Mips4ReusableBlockExecution::CounterSynchronization => {
                    self.advance_deferred_boundaries(&mut deferred_boundaries);
                    continue;
                }
                Mips4ReusableBlockExecution::Missing => {
                    self.advance_deferred_boundaries(&mut deferred_boundaries);
                    self.executor.target_mut().commit_block_frame(&mut frame);
                    if accumulated.boundaries != 0 {
                        return Ok(accumulated);
                    }
                    (
                        self.run_slice_with_code_window_inner(port, remaining, None, fast_memory)?,
                        false,
                        false,
                    )
                }
            };
            accumulated.retired_instructions += slice.retired_instructions;
            accumulated.boundaries += slice.boundaries;
            accumulated.exception_boundary =
                slice.exception_boundary.or(accumulated.exception_boundary);
            if persistent_frame {
                deferred_boundaries += slice.boundaries;
            }

            if counter_barrier {
                self.advance_deferred_boundaries(&mut deferred_boundaries);
                self.commit_reusable_block_frame(frame);
                accumulated.action = slice.action;
                return Ok(accumulated);
            }

            match slice.action {
                R5000ExecutionSliceAction::Progress
                    if accumulated.boundaries < budget
                        && accumulated.exception_boundary.is_none() =>
                {
                    if slice.boundaries == 0 {
                        no_progress_retries = no_progress_retries.saturating_add(1);
                        if no_progress_retries > 1 {
                            self.advance_deferred_boundaries(&mut deferred_boundaries);
                            return Err(self.block_error(format_args!(
                                "cached block dispatcher made no architectural progress with {remaining} boundaries requested, {} frame budget, {} frame retirements, and PC {:#018x}",
                                frame.budget(),
                                frame.retired(),
                                frame.pc(),
                            )));
                        }
                    } else {
                        no_progress_retries = 0;
                    }
                    if !persistent_frame {
                        frame =
                            self.take_block_frame(budget.saturating_sub(accumulated.boundaries));
                        block_key = self.executor.target().block_key_for_frame(&frame);
                        check_execution_readiness = true;
                    } else {
                        block_key.pc = frame.pc();
                        block_key.next_pc = frame.next_pc();
                        block_key.delay_slot_branch_pc = frame.delay_slot_branch_pc();
                    }
                }
                action => {
                    if persistent_frame {
                        self.advance_deferred_boundaries(&mut deferred_boundaries);
                        self.commit_reusable_block_frame(frame);
                    }
                    accumulated.action = action;
                    return Ok(accumulated);
                }
            }
        }
    }

    #[inline(never)]
    fn finish_cached_block_exit(
        &mut self,
        frame: &mut Mips4BlockFrame,
        previous_retired: u64,
        execution: Mips4BlockExecutionResult,
    ) -> Result<R5000ExecutionSlice, R5000CpuError> {
        let mut exception_boundary = None;
        let mut slice_action = R5000ExecutionSliceAction::Progress;
        match execution.exit {
            Mips4BlockExit::Exception => {
                match self.executor.target_mut().take_block_runtime_action() {
                    Some(ExecutionTargetAction::Boundary(boundary)) => {
                        exception_boundary = Some(boundary);
                    }
                    Some(_) => {
                        return Err(self.block_error(
                            "block runtime exception returned a non-boundary action",
                        ));
                    }
                    None => {
                        let exception = frame.exception().ok_or_else(|| {
                            self.block_error(
                                "block reported an exception without an exception code",
                            )
                        })?;
                        self.executor.target_mut().commit_block_control(frame);
                        exception_boundary =
                            Some(self.executor.target_mut().finish_block_exception(exception));
                        self.executor.target().refresh_block_control(frame);
                    }
                }
            }
            Mips4BlockExit::RuntimeTransaction => {
                let Some(ExecutionTargetAction::Transaction(transaction)) =
                    self.executor.target_mut().take_block_runtime_action()
                else {
                    return Err(self.block_error(
                        "block runtime transaction did not publish a transaction action",
                    ));
                };
                let transaction = self
                    .executor
                    .publish_ready_transaction(transaction)
                    .map_err(R5000CpuError::Execution)?;
                self.statistics.transactions = self.statistics.transactions.saturating_add(1);
                slice_action = R5000ExecutionSliceAction::Transaction(transaction);
            }
            Mips4BlockExit::RuntimeIdle => {
                slice_action = R5000ExecutionSliceAction::Idle;
            }
            Mips4BlockExit::InternalError => {
                return Err(self.block_error(format_args!(
                    "cached block execution returned an internal error at PC {:#018x} after {} operations",
                    frame.pc(), execution.operations_executed,
                )));
            }
            Mips4BlockExit::BudgetExhausted
            | Mips4BlockExit::Dispatch
            | Mips4BlockExit::TimelineExhausted
            | Mips4BlockExit::GuardInvalid => unreachable!("progress exits return above"),
        }

        let retired = frame
            .retired()
            .checked_sub(previous_retired)
            .ok_or_else(|| self.block_error("block retirement counter moved backwards"))?;
        let boundaries = retired + u64::from(exception_boundary.is_some());
        Ok(R5000ExecutionSlice {
            retired_instructions: retired,
            boundaries,
            exception_boundary,
            fast_fetches: 0,
            simulated_time_ticks: 0,
            action: slice_action,
        })
    }

    /// Runs a typed block with an optional versioned external code window.
    pub fn run_slice_with_code_window<P>(
        &mut self,
        port: &mut P,
        budget: u64,
        code_window: Option<&Mips4CodeWindow>,
    ) -> Result<R5000ExecutionSlice, R5000CpuError>
    where
        P: Mips4ExecutionPort,
        P::Error: fmt::Display,
    {
        let mut fast_memory = None;
        self.run_slice_with_code_window_inner(port, budget, code_window, &mut fast_memory)
    }

    fn run_slice_with_code_window_inner<P>(
        &mut self,
        port: &mut P,
        budget: u64,
        code_window: Option<&Mips4CodeWindow>,
        fast_memory: &mut Option<&mut P::FastMemoryRuntime>,
    ) -> Result<R5000ExecutionSlice, R5000CpuError>
    where
        P: Mips4ExecutionPort,
        P::Error: fmt::Display,
    {
        if let Some(error) = &self.terminal_error {
            return Err(error.clone());
        }
        if budget == 0 {
            return Ok(R5000ExecutionSlice {
                retired_instructions: 0,
                boundaries: 0,
                exception_boundary: None,
                fast_fetches: 0,
                simulated_time_ticks: 0,
                action: R5000ExecutionSliceAction::Progress,
            });
        }
        if !self.executor.ready_for_direct_execution()
            || !self.executor.target().block_execution_ready()
        {
            return self.poll_protocol_slice();
        }

        if self.executor.consume_ready_continuation()
            && !self.executor.target().dynamic_instruction_ready()
        {
            self.executor.target_mut().discard_dynamic_instruction();
        }
        if let Some(window) = code_window.filter(|window| {
            self.executor
                .target()
                .block_key_for_code_window(window)
                .is_some()
        }) {
            return self.run_stable_code_window_slice(port, budget, window, fast_memory);
        }

        let key = self.executor.target().block_key();
        let cached_ready = matches!(
            port.probe(
                key,
                Mips4BlockSource::InstructionCache,
                self.executor.target(),
            ),
            Mips4BlockProbe::Ready { .. }
        );
        if cached_ready {
            self.executor.target_mut().discard_dynamic_instruction();
        } else {
            let cached_block = self
                .executor
                .target()
                .build_block()
                .map_err(|error| self.block_error(error))?;
            if let Some(block) = cached_block {
                self.executor.target_mut().discard_dynamic_instruction();
                port.install(block, Mips4BlockSource::InstructionCache)
                    .map_err(|error| self.block_error(error))?;
            } else {
                if !self.executor.target().dynamic_instruction_ready() {
                    self.executor.target_mut().discard_dynamic_instruction();
                    let fetch_action =
                        self.executor
                            .target_mut()
                            .begin_block_fetch()
                            .map_err(|error| {
                                R5000CpuError::Execution(FunctionalExecutorError::Target(error))
                            })?;
                    match fetch_action {
                        ExecutionTargetAction::Continue => {}
                        action => {
                            let action = self
                                .executor
                                .publish_ready_action(action)
                                .map_err(R5000CpuError::Execution)?;
                            return self.accelerated_protocol_slice(action);
                        }
                    }
                }
                let block = self
                    .executor
                    .target_mut()
                    .take_dynamic_block()
                    .map_err(|error| self.block_error(error))?
                    .ok_or_else(|| {
                        self.block_error(
                            "instruction fetch continuation did not provide a dynamic block",
                        )
                    })?;
                port.install(block, Mips4BlockSource::DynamicFetch)
                    .map_err(|error| self.block_error(error))?;
            }
        }

        let mut frame = self.take_block_frame(budget);
        let execution = {
            let target = self.executor.target_mut();
            port.execute(key, &mut frame, target, reborrow_optional(fast_memory))
                .map_err(|error| self.block_error(error))?
        };
        self.executor.target_mut().commit_block_frame(&mut frame);
        let fast_fetches = 0;

        let mut exception_boundary = None;
        let mut slice_action = R5000ExecutionSliceAction::Progress;
        match execution.exit {
            Mips4BlockExit::BudgetExhausted | Mips4BlockExit::Dispatch => {}
            Mips4BlockExit::Exception => {
                match self.executor.target_mut().take_block_runtime_action() {
                    Some(ExecutionTargetAction::Boundary(boundary)) => {
                        exception_boundary = Some(boundary);
                    }
                    Some(_) => {
                        return Err(self.block_error(
                            "block runtime exception returned a non-boundary action",
                        ));
                    }
                    None => {
                        let exception = frame.exception().ok_or_else(|| {
                            self.block_error(
                                "block reported an exception without an exception code",
                            )
                        })?;
                        exception_boundary =
                            Some(self.executor.target_mut().finish_block_exception(exception));
                    }
                }
            }
            Mips4BlockExit::RuntimeTransaction => {
                let Some(ExecutionTargetAction::Transaction(transaction)) =
                    self.executor.target_mut().take_block_runtime_action()
                else {
                    return Err(self.block_error(
                        "block runtime transaction did not publish a transaction action",
                    ));
                };
                let transaction = self
                    .executor
                    .publish_ready_transaction(transaction)
                    .map_err(R5000CpuError::Execution)?;
                self.statistics.transactions = self.statistics.transactions.saturating_add(1);
                slice_action = R5000ExecutionSliceAction::Transaction(transaction);
            }
            Mips4BlockExit::RuntimeIdle => {
                slice_action = R5000ExecutionSliceAction::Idle;
            }
            Mips4BlockExit::TimelineExhausted => {}
            Mips4BlockExit::GuardInvalid => {}
            Mips4BlockExit::InternalError => {
                return Err(self.block_error(format_args!(
                    "dynamic block execution returned an internal error for {key:?} at PC {:#018x} after {} operations",
                    frame.pc(), execution.operations_executed,
                )));
            }
        }
        let retired = frame.retired();
        let boundaries = retired + u64::from(exception_boundary.is_some());
        self.statistics.retired_instructions =
            self.statistics.retired_instructions.saturating_add(retired);
        if exception_boundary.is_some() {
            self.statistics.exceptions = self.statistics.exceptions.saturating_add(1);
            self.executor.target().refresh_block_control(&mut frame);
        }
        self.executor.target_mut().advance_random(boundaries);
        self.advance_pclocks(boundaries);
        let simulated_time_ticks = 0;
        self.reusable_block_frame.0 = Some(frame);
        Ok(R5000ExecutionSlice {
            retired_instructions: retired,
            boundaries,
            exception_boundary,
            fast_fetches,
            simulated_time_ticks,
            action: slice_action,
        })
    }

    fn run_stable_code_window_slice<P>(
        &mut self,
        port: &mut P,
        budget: u64,
        window: &Mips4CodeWindow,
        fast_memory: &mut Option<&mut P::FastMemoryRuntime>,
    ) -> Result<R5000ExecutionSlice, R5000CpuError>
    where
        P: Mips4ExecutionPort,
        P::Error: fmt::Display,
    {
        let fetch_limit = window.fetch_count() as u64;
        let mut frame = self.take_block_frame(budget.min(fetch_limit));
        let mut fast_fetches = 0_u64;
        let mut final_block = None;

        loop {
            let remaining_fetches = fetch_limit.saturating_sub(fast_fetches);
            if remaining_fetches == 0 || frame.budget() == 0 {
                break;
            }
            frame.limit_budget(remaining_fetches);
            let Some(key) = self
                .executor
                .target()
                .block_key_for_code_window_frame(window, &frame)
            else {
                break;
            };
            let counter_barrier = match port.probe(
                key,
                Mips4BlockSource::Stable(window.guard()),
                self.executor.target(),
            ) {
                Mips4BlockProbe::Ready { counter_barrier } => counter_barrier,
                Mips4BlockProbe::Missing => {
                    let block = self
                        .executor
                        .target()
                        .build_code_window_for_key(window, key)
                        .map_err(|error| self.block_error(error))?
                        .ok_or_else(|| {
                            self.block_error("stable code window did not produce a successor block")
                        })?;
                    port.install(block, Mips4BlockSource::Stable(window.guard()))
                        .map_err(|error| self.block_error(error))?;
                    match port.probe(
                        key,
                        Mips4BlockSource::Stable(window.guard()),
                        self.executor.target(),
                    ) {
                        Mips4BlockProbe::Ready { counter_barrier } => counter_barrier,
                        Mips4BlockProbe::Missing => {
                            return Err(self.block_error("installed stable block was not reusable"));
                        }
                    }
                }
            };
            if frame.retired() != 0 && counter_barrier {
                break;
            }

            self.executor.target_mut().discard_dynamic_instruction();
            let execution = {
                let target = self.executor.target_mut();
                port.execute(key, &mut frame, target, reborrow_optional(fast_memory))
                    .map_err(|error| self.block_error(error))?
            };
            fast_fetches = fast_fetches
                .checked_add(execution.operations_executed)
                .ok_or_else(|| self.block_error("stable fetch counter overflowed"))?;
            final_block = Some((key, execution));

            if execution.counter_barrier
                || !matches!(
                    execution.exit,
                    Mips4BlockExit::BudgetExhausted | Mips4BlockExit::Dispatch
                )
            {
                break;
            }
        }

        let Some((key, execution)) = final_block else {
            return Err(self.block_error("stable code window made no architectural progress"));
        };
        self.executor.target_mut().commit_block_frame(&mut frame);
        self.executor
            .account_ready_transactions(fast_fetches)
            .map_err(R5000CpuError::Execution)?;
        self.statistics.transactions = self.statistics.transactions.saturating_add(fast_fetches);

        let mut exception_boundary = None;
        let mut slice_action = R5000ExecutionSliceAction::Progress;
        match execution.exit {
            Mips4BlockExit::BudgetExhausted | Mips4BlockExit::Dispatch => {}
            Mips4BlockExit::Exception => {
                match self.executor.target_mut().take_block_runtime_action() {
                    Some(ExecutionTargetAction::Boundary(boundary)) => {
                        exception_boundary = Some(boundary);
                    }
                    Some(_) => {
                        return Err(self.block_error(
                            "block runtime exception returned a non-boundary action",
                        ));
                    }
                    None => {
                        let exception = frame.exception().ok_or_else(|| {
                            self.block_error(
                                "block reported an exception without an exception code",
                            )
                        })?;
                        exception_boundary =
                            Some(self.executor.target_mut().finish_block_exception(exception));
                    }
                }
            }
            Mips4BlockExit::RuntimeTransaction => {
                let Some(ExecutionTargetAction::Transaction(transaction)) =
                    self.executor.target_mut().take_block_runtime_action()
                else {
                    return Err(self.block_error(
                        "block runtime transaction did not publish a transaction action",
                    ));
                };
                let transaction = self
                    .executor
                    .publish_ready_transaction(transaction)
                    .map_err(R5000CpuError::Execution)?;
                self.statistics.transactions = self.statistics.transactions.saturating_add(1);
                slice_action = R5000ExecutionSliceAction::Transaction(transaction);
            }
            Mips4BlockExit::RuntimeIdle => {
                slice_action = R5000ExecutionSliceAction::Idle;
            }
            Mips4BlockExit::TimelineExhausted => {}
            Mips4BlockExit::GuardInvalid => {}
            Mips4BlockExit::InternalError => {
                return Err(self.block_error(format_args!(
                    "stable block execution returned an internal error for {key:?} at PC {:#018x} after {} operations",
                    frame.pc(), execution.operations_executed,
                )));
            }
        }
        let retired = frame.retired();
        let boundaries = retired + u64::from(exception_boundary.is_some());
        self.statistics.retired_instructions =
            self.statistics.retired_instructions.saturating_add(retired);
        if exception_boundary.is_some() {
            self.statistics.exceptions = self.statistics.exceptions.saturating_add(1);
            self.executor.target().refresh_block_control(&mut frame);
        }
        self.executor.target_mut().advance_random(boundaries);
        self.advance_pclocks(boundaries);
        let simulated_time_ticks = window
            .fetch_time_ticks(fast_fetches as usize)
            .ok_or_else(|| self.block_error("stable fetches exceeded their planned timeline"))?;
        self.reusable_block_frame.0 = Some(frame);
        Ok(R5000ExecutionSlice {
            retired_instructions: retired,
            boundaries,
            exception_boundary,
            fast_fetches,
            simulated_time_ticks,
            action: slice_action,
        })
    }

    fn poll_protocol_slice(&mut self) -> Result<R5000ExecutionSlice, R5000CpuError> {
        let action = self.poll()?;
        Ok(match action {
            ExecutionAction::Transaction(transaction) => R5000ExecutionSlice {
                retired_instructions: 0,
                boundaries: 0,
                exception_boundary: None,
                fast_fetches: 0,
                simulated_time_ticks: 0,
                action: R5000ExecutionSliceAction::Transaction(transaction),
            },
            ExecutionAction::Boundary(Mips4ExecutionBoundary::Retired { .. }) => {
                R5000ExecutionSlice {
                    retired_instructions: 1,
                    boundaries: 1,
                    exception_boundary: None,
                    fast_fetches: 0,
                    simulated_time_ticks: 0,
                    action: R5000ExecutionSliceAction::Progress,
                }
            }
            ExecutionAction::Boundary(boundary) => R5000ExecutionSlice {
                retired_instructions: 0,
                boundaries: 1,
                exception_boundary: Some(boundary),
                fast_fetches: 0,
                simulated_time_ticks: 0,
                action: R5000ExecutionSliceAction::Progress,
            },
            ExecutionAction::Idle => R5000ExecutionSlice {
                retired_instructions: 0,
                boundaries: 0,
                exception_boundary: None,
                fast_fetches: 0,
                simulated_time_ticks: 0,
                action: R5000ExecutionSliceAction::Idle,
            },
            ExecutionAction::Waiting { transaction_id } => R5000ExecutionSlice {
                retired_instructions: 0,
                boundaries: 0,
                exception_boundary: None,
                fast_fetches: 0,
                simulated_time_ticks: 0,
                action: R5000ExecutionSliceAction::Waiting { transaction_id },
            },
        })
    }

    fn accelerated_protocol_slice(
        &mut self,
        action: ExecutionAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary>,
    ) -> Result<R5000ExecutionSlice, R5000CpuError> {
        let slice = match action {
            ExecutionAction::Transaction(transaction) => {
                self.statistics.transactions = self.statistics.transactions.saturating_add(1);
                R5000ExecutionSlice {
                    retired_instructions: 0,
                    boundaries: 0,
                    exception_boundary: None,
                    fast_fetches: 0,
                    simulated_time_ticks: 0,
                    action: R5000ExecutionSliceAction::Transaction(transaction),
                }
            }
            ExecutionAction::Boundary(boundary @ Mips4ExecutionBoundary::Retired { .. }) => {
                if let Some(frame) = &mut self.reusable_block_frame.0 {
                    self.executor
                        .target()
                        .refresh_block_boundary(frame, &boundary);
                }
                self.statistics.retired_instructions =
                    self.statistics.retired_instructions.saturating_add(1);
                self.executor.target_mut().advance_random(1);
                self.advance_pclocks(1);
                R5000ExecutionSlice {
                    retired_instructions: 1,
                    boundaries: 1,
                    exception_boundary: None,
                    fast_fetches: 0,
                    simulated_time_ticks: 0,
                    action: R5000ExecutionSliceAction::Progress,
                }
            }
            ExecutionAction::Boundary(boundary) => {
                if let Some(frame) = &mut self.reusable_block_frame.0 {
                    self.executor
                        .target()
                        .refresh_block_boundary(frame, &boundary);
                }
                self.statistics.exceptions = self.statistics.exceptions.saturating_add(1);
                self.executor.target_mut().advance_random(1);
                self.advance_pclocks(1);
                R5000ExecutionSlice {
                    retired_instructions: 0,
                    boundaries: 1,
                    exception_boundary: Some(boundary),
                    fast_fetches: 0,
                    simulated_time_ticks: 0,
                    action: R5000ExecutionSliceAction::Progress,
                }
            }
            ExecutionAction::Idle => R5000ExecutionSlice {
                retired_instructions: 0,
                boundaries: 0,
                exception_boundary: None,
                fast_fetches: 0,
                simulated_time_ticks: 0,
                action: R5000ExecutionSliceAction::Idle,
            },
            ExecutionAction::Waiting { transaction_id } => R5000ExecutionSlice {
                retired_instructions: 0,
                boundaries: 0,
                exception_boundary: None,
                fast_fetches: 0,
                simulated_time_ticks: 0,
                action: R5000ExecutionSliceAction::Waiting { transaction_id },
            },
        };
        Ok(slice)
    }

    #[cold]
    #[inline(never)]
    fn block_error(&mut self, error: impl fmt::Display) -> R5000CpuError {
        let error = R5000CpuError::Block(error.to_string());
        self.terminal_error = Some(error.clone());
        error
    }

    fn advance_deferred_boundaries(&mut self, deferred_boundaries: &mut u64) {
        let boundaries = core::mem::take(deferred_boundaries);
        if boundaries == 0 {
            return;
        }
        self.executor.target_mut().advance_random(boundaries);
        self.advance_pclocks(boundaries);
    }

    fn take_block_frame(&mut self, budget: u64) -> Mips4BlockFrame {
        if let Some(mut frame) = self.reusable_block_frame.0.take() {
            debug_assert_eq!(frame.pc(), self.state().pc());
            debug_assert_eq!(frame.next_pc(), self.state().next_pc());
            debug_assert_eq!(
                frame.delay_slot_branch_pc(),
                self.state().delay_slot_branch_pc()
            );
            debug_assert_eq!(frame.hi(), self.state().hi());
            debug_assert_eq!(frame.lo(), self.state().lo());
            frame.prepare(budget);
            frame
        } else {
            self.executor.target().block_frame(budget)
        }
    }

    fn commit_reusable_block_frame(&mut self, mut frame: Mips4BlockFrame) {
        self.executor.target_mut().commit_block_frame(&mut frame);
        self.reusable_block_frame.0 = Some(frame);
    }

    /// Advances Count using machine-calculated elapsed processor clocks.
    pub fn advance_pclocks(&mut self, pclocks: u64) {
        let count_increments = match self.boot_mode.count_update_rate() {
            R5000CountUpdateRate::PClock => pclocks,
            R5000CountUpdateRate::HalfPClock => {
                let low = (pclocks & 1) as u8 + self.half_pclock_remainder;
                self.half_pclock_remainder = low & 1;
                (pclocks >> 1) + u64::from(low >> 1)
            }
        };
        self.executor
            .target_mut()
            .advance_count(count_increments, self.boot_mode.timer_interrupt_enabled());
    }

    /// Caps a slice at the first Count/Compare matching processor-clock boundary.
    pub fn limit_slice_budget(&self, requested: u64) -> u64 {
        if requested == 0 || !self.boot_mode.timer_interrupt_enabled() {
            return requested;
        }
        let count = self.state().cp0().count().bits();
        let compare = self.state().cp0().compare().bits();
        let increments = compare.wrapping_sub(count);
        if increments == 0 {
            return requested;
        }
        let boundary = match self.boot_mode.count_update_rate() {
            R5000CountUpdateRate::PClock => u64::from(increments),
            R5000CountUpdateRate::HalfPClock => u64::from(increments)
                .saturating_mul(2)
                .saturating_sub(u64::from(self.half_pclock_remainder)),
        };
        requested.min(boundary)
    }
}

impl<F> Component for R5000Cpu<F>
where
    F: FloatBackend + 'static,
{
    fn id(&self) -> ComponentId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn reset(&mut self) {
        self.executor.reset();
        self.half_pclock_remainder = 0;
        self.terminal_error = None;
        self.reusable_block_frame = R5000ReusableBlockFrame::default();
    }
}

impl<F> BusControllerRole<ExecutionCompletion<Mips4ExecutionCompletion>> for R5000Cpu<F>
where
    F: FloatBackend,
{
    fn complete(&mut self, completion: ExecutionCompletion<Mips4ExecutionCompletion>) {
        if self.terminal_error.is_some() {
            return;
        }
        if let Err(error) = self.executor.complete(completion) {
            self.terminal_error = Some(R5000CpuError::Execution(error));
        }
    }
}

impl<F> BusDeviceRole<R5000CpuSignal> for R5000Cpu<F>
where
    F: FloatBackend,
{
    type Response = ();

    fn accept(&mut self, signal: R5000CpuSignal) {
        let signal = match signal {
            R5000CpuSignal::InvalidateReservation => Mips4ExecutionSignal::InvalidateReservation,
            R5000CpuSignal::SoftReset => Mips4ExecutionSignal::SoftReset,
            R5000CpuSignal::NonMaskableInterrupt => Mips4ExecutionSignal::NonMaskableInterrupt,
            R5000CpuSignal::CacheError(cache_error) => {
                Mips4ExecutionSignal::CacheError(cache_error)
            }
        };
        self.executor.signal(signal);
    }
}

impl<F> BusDeviceRole<IrqDelivery> for R5000Cpu<F>
where
    F: FloatBackend,
{
    type Response = Result<(), R5000IrqError>;

    fn accept(&mut self, delivery: IrqDelivery) -> Self::Response {
        let input = delivery.input.get();
        if !(R5000_IRQ_IP2.get()..=R5000_IRQ_IP6.get()).contains(&input) {
            return Err(R5000IrqError::UnsupportedInput(delivery.input));
        }
        let mask = 1_u8 << input;
        let current = self.state().external_interrupts();
        let levels = if delivery.asserted {
            current | mask
        } else {
            current & !mask
        };
        self.executor
            .signal(Mips4ExecutionSignal::ExternalInterrupts(levels));
        Ok(())
    }
}

#[cfg(test)]
mod tests;
