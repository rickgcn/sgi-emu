//! Persistent runtime state outside component ownership.

use se_core::scheduler::state::{SchedulerState, SchedulerStateError};

use super::{Runtime, RuntimeStatistics};

/// Runtime scheduler, stop, and diagnostic state.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct RuntimeState<E> {
    scheduler: SchedulerState<E>,
    stopped: bool,
    statistics: RuntimeStatistics,
}

impl<E> RuntimeState<E> {
    /// Returns the saved scheduler state.
    pub const fn scheduler(&self) -> &SchedulerState<E> {
        &self.scheduler
    }

    /// Returns whether the runtime was stopped.
    pub const fn stopped(&self) -> bool {
        self.stopped
    }

    /// Returns cumulative runtime counters.
    pub const fn statistics(&self) -> RuntimeStatistics {
        self.statistics
    }
}

impl<E, S> Runtime<E, S> {
    /// Captures scheduler and runtime bookkeeping without the registry or trace sink.
    pub fn save_state(&self) -> RuntimeState<E>
    where
        E: Clone,
    {
        RuntimeState {
            scheduler: self.scheduler.save_state(),
            stopped: self.stopped,
            statistics: self.statistics,
        }
    }

    /// Restores scheduler and runtime bookkeeping without changing components or tracing.
    pub fn restore_state(&mut self, state: RuntimeState<E>) -> Result<(), SchedulerStateError> {
        self.scheduler.restore_state(state.scheduler)?;
        self.stopped = state.stopped;
        self.statistics = state.statistics;
        Ok(())
    }
}
