//! Runtime shell for the SGI Indigo IP12 machine profile.
//!
//! The machine shell owns an event-driven runtime specialized to `Ip12Event`.
//! Its dispatch function handles machine-level control events.

use core::convert::Infallible;

use se_core::scheduler::{ScheduledEvent, ScheduledEventId, SchedulerError, SimTime};
use se_core::tracing::{NoopTraceSink, TraceSink};
use se_runtime::registry::ComponentRegistry;
use se_runtime::runtime::{RunError, RunStatus, Runtime, RuntimeContext};

use super::component_ids;
use super::event::Ip12Event;

/// SGI Indigo IP12 machine shell.
///
/// Mutable runtime access is intentionally unavailable so machine-level
/// invariants remain owned by this type.
///
/// ```compile_fail
/// use se_machine::indigo::ip12::machine::Ip12Machine;
///
/// let mut machine = Ip12Machine::new();
/// let _ = machine.runtime_mut();
/// ```
pub struct Ip12Machine<S = NoopTraceSink> {
    runtime: Runtime<Ip12Event, S>,
}

impl Ip12Machine<NoopTraceSink> {
    /// Creates an IP12 machine shell with a noop trace sink.
    pub fn new() -> Self {
        Self::with_trace_sink(NoopTraceSink)
    }
}

impl Default for Ip12Machine<NoopTraceSink> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Ip12Machine<S> {
    /// Creates an IP12 machine shell with the given trace sink.
    pub fn with_trace_sink(sink: S) -> Self {
        Self {
            runtime: Runtime::with_trace_sink(sink),
        }
    }

    /// Returns an immutable runtime reference.
    pub const fn runtime(&self) -> &Runtime<Ip12Event, S> {
        &self.runtime
    }

    /// Consumes the machine shell and returns the owned runtime.
    pub fn into_runtime(self) -> Runtime<Ip12Event, S> {
        self.runtime
    }
}

impl<S> Ip12Machine<S>
where
    S: TraceSink,
{
    /// Schedules the power-on event at simulated time zero.
    pub fn schedule_power_on(&mut self) -> Result<ScheduledEventId, SchedulerError> {
        self.runtime
            .schedule_at(SimTime::ZERO, component_ids::MACHINE, Ip12Event::PowerOn)
    }

    /// Schedules a reset event at the current simulated time.
    pub fn schedule_reset(&mut self) -> Result<ScheduledEventId, SchedulerError> {
        self.runtime
            .schedule_at(self.runtime.now(), component_ids::MACHINE, Ip12Event::Reset)
    }

    /// Runs the IP12 machine until the requested simulated-time deadline.
    pub fn run_until_time(&mut self, deadline: SimTime) -> Result<RunStatus, RunError<Infallible>> {
        self.runtime.run_until_time(deadline, dispatch_event::<S>)
    }
}

fn dispatch_event<S>(
    event: ScheduledEvent<Ip12Event>,
    _registry: &mut ComponentRegistry,
    _context: &mut RuntimeContext<'_, Ip12Event, S>,
) -> Result<(), Infallible> {
    match event.payload {
        Ip12Event::PowerOn | Ip12Event::Reset => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests;
