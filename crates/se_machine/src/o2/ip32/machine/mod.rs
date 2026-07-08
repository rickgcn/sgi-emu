//! Runtime shell for the SGI O2 IP32 machine profile.
//!
//! The machine shell owns an event-driven runtime specialized to `Ip32Event`.
//! Its dispatch function handles machine-level control events.

use core::convert::Infallible;

use se_core::scheduler::{ScheduledEvent, ScheduledEventId, SchedulerError, SimTime};
use se_core::tracing::{NoopTraceSink, TraceSink};
use se_runtime::registry::ComponentRegistry;
use se_runtime::runtime::{RunError, RunStatus, Runtime, RuntimeContext};

use super::component_ids;
use super::event::Ip32Event;

/// SGI O2 IP32 machine shell.
pub struct Ip32Machine<S = NoopTraceSink> {
    runtime: Runtime<Ip32Event, S>,
}

impl Ip32Machine<NoopTraceSink> {
    /// Creates an IP32 machine shell with a noop trace sink.
    pub fn new() -> Self {
        Self::with_trace_sink(NoopTraceSink)
    }
}

impl Default for Ip32Machine<NoopTraceSink> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Ip32Machine<S> {
    /// Creates an IP32 machine shell with the given trace sink.
    pub fn with_trace_sink(sink: S) -> Self {
        Self {
            runtime: Runtime::with_trace_sink(sink),
        }
    }

    /// Returns an immutable runtime reference.
    pub const fn runtime(&self) -> &Runtime<Ip32Event, S> {
        &self.runtime
    }

    /// Returns a mutable runtime reference.
    pub const fn runtime_mut(&mut self) -> &mut Runtime<Ip32Event, S> {
        &mut self.runtime
    }

    /// Consumes the machine shell and returns the owned runtime.
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

    /// Runs the IP32 machine until the requested simulated-time deadline.
    pub fn run_until_time(&mut self, deadline: SimTime) -> Result<RunStatus, RunError<Infallible>> {
        self.runtime.run_until_time(deadline, dispatch_event::<S>)
    }
}

fn dispatch_event<S>(
    event: ScheduledEvent<Ip32Event>,
    _registry: &mut ComponentRegistry,
    _context: &mut RuntimeContext<'_, Ip32Event, S>,
) -> Result<(), Infallible> {
    match event.payload {
        Ip32Event::PowerOn | Ip32Event::Reset => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests;
