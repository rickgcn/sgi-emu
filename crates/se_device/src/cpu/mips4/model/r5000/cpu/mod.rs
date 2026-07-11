//! Functional R5000 CPU component and bus roles.

use core::fmt;

use se_core::component::{Component, ComponentId};
use se_core::role::{BusControllerRole, BusDeviceRole};
use se_float::backend::FloatBackend;
use se_float::backend::softfloat3::SoftFloat3Backend;

use crate::cpu::execution::functional::FunctionalExecutor;
use crate::cpu::execution::protocol::{
    ExecutionAction, ExecutionCompletion, FunctionalExecutorError,
};
use crate::cpu::mips4::execution::bus::{Mips4ExecutionCompletion, Mips4ExecutionTransaction};
use crate::cpu::mips4::execution::state::Mips4ExecutionState;
use crate::cpu::mips4::execution::target::{
    Mips4ExecutionBoundary, Mips4ExecutionSignal, Mips4ExecutionTarget, Mips4ExecutionTargetError,
};

use super::boot_mode::{R5000BootMode, R5000CountUpdateRate};
use super::execution_policy::R5000ExecutionPolicy;
use super::profile::R5000Profile;

/// External signal delivered to the R5000 through its bus-device role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum R5000CpuSignal {
    /// Replaces the external hardware interrupt levels.
    ExternalInterrupts(u8),

    /// Invalidates an outstanding load-linked reservation.
    InvalidateReservation,
}

/// Terminal functional R5000 execution error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum R5000CpuError {
    /// The generic executor or MIPS IV target rejected an operation.
    Execution(FunctionalExecutorError<Mips4ExecutionTargetError>),
}

impl fmt::Display for R5000CpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Execution(error) => write!(f, "R5000 execution failed: {error}"),
        }
    }
}

impl std::error::Error for R5000CpuError {}

/// Functional R5000 CPU with an injectable floating-point backend.
pub struct R5000Cpu<F = SoftFloat3Backend>
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
}

impl R5000Cpu<SoftFloat3Backend> {
    /// Creates an R5000 using SoftFloat 3e as its reference FPU backend.
    pub fn new(
        id: ComponentId,
        name: impl Into<String>,
        profile: R5000Profile,
        boot_mode: R5000BootMode,
    ) -> Self {
        Self::with_float_backend(id, name, profile, boot_mode, SoftFloat3Backend::new())
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
    ) -> Self {
        let policy = R5000ExecutionPolicy::new(profile, boot_mode);
        Self {
            id,
            name: name.into(),
            profile,
            boot_mode,
            executor: FunctionalExecutor::new(Mips4ExecutionTarget::new(policy, float_backend)),
            half_pclock_remainder: 0,
            terminal_error: None,
        }
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

    /// Polls the next transaction, instruction boundary, or wait state.
    pub fn poll(
        &mut self,
    ) -> Result<ExecutionAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary>, R5000CpuError>
    {
        if let Some(error) = &self.terminal_error {
            return Err(error.clone());
        }
        let action = self.executor.poll().map_err(R5000CpuError::Execution)?;
        if matches!(action, ExecutionAction::Boundary(_)) {
            self.executor.target_mut().advance_random(1);
            self.advance_pclocks(1);
        }
        Ok(action)
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
}

impl<F> Component for R5000Cpu<F>
where
    F: FloatBackend,
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
            R5000CpuSignal::ExternalInterrupts(levels) => {
                Mips4ExecutionSignal::ExternalInterrupts(levels)
            }
            R5000CpuSignal::InvalidateReservation => Mips4ExecutionSignal::InvalidateReservation,
        };
        self.executor.signal(signal);
    }
}

#[cfg(test)]
mod tests;
