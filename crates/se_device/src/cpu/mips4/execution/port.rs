//! Portable MIPS IV block-execution port contracts.

use core::fmt;

use super::block::{
    Mips4Block, Mips4BlockExit, Mips4BlockFrame, Mips4BlockKey, Mips4BlockRuntime, Mips4CodeGuard,
    Mips4FastMemoryRuntime,
};

/// Instruction source associated with one translated block.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4BlockSource {
    /// A block derived from modeled instruction-cache contents.
    InstructionCache,
    /// A block derived from one architecturally completed fetch transaction.
    DynamicFetch,
    /// A block derived from a versioned side-effect-free code window.
    Stable(Mips4CodeGuard),
}

/// Result of probing an execution port for a translated block.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4BlockProbe {
    /// No reusable translated block is available.
    Missing,
    /// A translated block is ready for execution.
    Ready {
        /// Whether deferred CP0 counter updates must be synchronized first.
        counter_barrier: bool,
    },
}

/// Portable result of one translated block invocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4BlockExecutionResult {
    /// Architectural reason execution returned to the CPU dispatcher.
    pub exit: Mips4BlockExit,
    /// Whether the block observes or changes dynamic CP0 counter state.
    pub counter_barrier: bool,
    /// Guest operations entered by this invocation.
    pub operations_executed: u64,
}

/// Result of attempting one reusable translated block.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4ReusableBlockExecution {
    /// No valid reusable block exists for the requested key.
    Missing,
    /// Deferred CP0 counters must be committed before executing the block.
    CounterSynchronization,
    /// A reusable block executed normally.
    Executed(Mips4BlockExecutionResult),
}

/// Abstract translated-execution service consumed by a MIPS IV CPU.
pub trait Mips4ExecutionPort {
    /// Port-owned installation or execution failure.
    type Error: fmt::Display;

    /// Optional fast-memory runtime accepted by this port.
    type FastMemoryRuntime: Mips4FastMemoryRuntime + ?Sized;

    /// Probes for a valid translated block matching the requested source.
    fn probe<R>(
        &mut self,
        key: Mips4BlockKey,
        source: Mips4BlockSource,
        runtime: &R,
    ) -> Mips4BlockProbe
    where
        R: Mips4BlockRuntime + ?Sized;

    /// Installs a verified translated block with its source identity.
    fn install(&mut self, block: Mips4Block, source: Mips4BlockSource) -> Result<(), Self::Error>;

    /// Executes one installed translated block.
    fn execute<R>(
        &mut self,
        key: Mips4BlockKey,
        frame: &mut Mips4BlockFrame,
        runtime: &mut R,
        fast_memory: Option<&mut Self::FastMemoryRuntime>,
    ) -> Result<Mips4BlockExecutionResult, Self::Error>
    where
        R: Mips4BlockRuntime;

    /// Executes one reusable instruction-cache block with a single lookup.
    fn execute_reusable<R>(
        &mut self,
        key: Mips4BlockKey,
        frame: &mut Mips4BlockFrame,
        runtime: &mut R,
        fast_memory: Option<&mut Self::FastMemoryRuntime>,
        counters_dirty: bool,
    ) -> Result<Mips4ReusableBlockExecution, Self::Error>
    where
        R: Mips4BlockRuntime;
}
