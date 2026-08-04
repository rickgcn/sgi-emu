//! Portable MIPS IV block-execution port contracts.

use core::fmt;

use super::block::{
    Mips4Block, Mips4BlockExit, Mips4BlockFrame, Mips4BlockKey, Mips4BlockRuntime, Mips4CodeGuard,
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

/// Reason a reusable cached-block batch stopped after making progress.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4ReusableBatchStop {
    /// The last executed block produced an exit requiring CPU dispatch.
    BlockExit,
    /// The successor selected by the last block was not reusable.
    MissingSuccessor,
    /// Deferred CP0 counters must be synchronized before the successor.
    CounterSynchronization,
}

/// Aggregate result of one reusable cached-block batch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4ReusableBatchResult {
    /// Architectural result of the final executed block.
    pub execution: Mips4BlockExecutionResult,
    /// Reason execution returned after at least one entry.
    pub stop: Mips4ReusableBatchStop,
    /// Number of reusable block or Region entries consumed by the batch.
    pub entries: u64,
}

/// Result of attempting one reusable cached-block batch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4ReusableBatchExecution {
    /// No valid reusable block exists for the requested entry key.
    Missing,
    /// Deferred CP0 counters must be synchronized before the entry block.
    CounterSynchronization,
    /// One or more reusable entries executed.
    Executed(Mips4ReusableBatchResult),
}

/// Abstract translated-execution service consumed by a MIPS IV CPU.
pub trait Mips4ExecutionPort {
    /// Port-owned installation or execution failure.
    type Error: fmt::Display;

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
    ) -> Result<Mips4BlockExecutionResult, Self::Error>
    where
        R: Mips4BlockRuntime;

    /// Executes one reusable instruction-cache block with a single lookup.
    fn execute_reusable<R>(
        &mut self,
        key: Mips4BlockKey,
        frame: &mut Mips4BlockFrame,
        runtime: &mut R,
        counters_dirty: bool,
    ) -> Result<Mips4ReusableBlockExecution, Self::Error>
    where
        R: Mips4BlockRuntime;

    /// Executes a reusable cached-block batch.
    ///
    /// Ports without an internal dispatcher retain single-entry behavior.
    fn execute_reusable_batch<R>(
        &mut self,
        key: Mips4BlockKey,
        frame: &mut Mips4BlockFrame,
        runtime: &mut R,
        counters_dirty: bool,
    ) -> Result<Mips4ReusableBatchExecution, Self::Error>
    where
        R: Mips4BlockRuntime,
    {
        self.execute_reusable(key, frame, runtime, counters_dirty)
            .map(|execution| match execution {
                Mips4ReusableBlockExecution::Missing => Mips4ReusableBatchExecution::Missing,
                Mips4ReusableBlockExecution::CounterSynchronization => {
                    Mips4ReusableBatchExecution::CounterSynchronization
                }
                Mips4ReusableBlockExecution::Executed(execution) => {
                    Mips4ReusableBatchExecution::Executed(Mips4ReusableBatchResult {
                        execution,
                        stop: Mips4ReusableBatchStop::BlockExit,
                        entries: 1,
                    })
                }
            })
    }
}
