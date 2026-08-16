//! Implements the single-threaded deterministic machine drive loop.
//!
//! A finite bound limits the furthest CPU deadline but never replaces an earlier
//! event horizon. Every successful CPU return invalidates its deadline. Deadline
//! exits drain the current virtual instant to quiescence; all continuing paths
//! then query the machine for a new event horizon.

use std::io::{Read, Seek, Write};

use se_core::machine::{CpuExit, MachineError, StateDigest};
use se_core::snapshot::{BuildFingerprint, read_snapshot, write_snapshot};
use se_core::time::{NO_DEADLINE, VTime};

use crate::{PauseReason, RunOutcome, Runtime, RuntimeError, RuntimeState};

impl Runtime {
    /// Creates a paused runtime around the factory's canonical initial machine.
    ///
    /// The factory profile identity is obtained before machine construction. No
    /// guest CPU work or event dispatch occurs during construction.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::MachineCreate`] if the factory cannot construct the
    /// initial machine.
    pub fn new(
        factory: Box<dyn se_core::machine::MachineFactory>,
        build: BuildFingerprint,
    ) -> Result<Self, RuntimeError> {
        let _ = factory.profile_fingerprint();
        let machine = factory.create().map_err(RuntimeError::MachineCreate)?;
        Ok(Self {
            machine,
            factory,
            build,
            state: RuntimeState::Paused,
            host_pause_requests: 0,
        })
    }

    /// Returns the current host-visible runtime state.
    #[must_use]
    pub const fn state(&self) -> RuntimeState {
        self.state
    }

    /// Returns the current machine virtual time in nanoseconds.
    #[must_use]
    pub fn now(&self) -> VTime {
        self.machine.now()
    }

    /// Drives the machine until it pauses, reaches a breakpoint, or halts.
    ///
    /// This call may remain inside an unbounded CPU burst when no finite event or
    /// stop condition exists. It reads no host wall clock.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::InvalidState`] unless the runtime is paused. A
    /// machine drive or event-dispatch failure returns [`RuntimeError::Machine`]
    /// and changes the runtime state to [`RuntimeState::Faulted`].
    pub fn run(&mut self) -> Result<RunOutcome, RuntimeError> {
        self.begin_run("run")?;
        self.drive(None)
    }

    /// Drives the machine no later than `target` virtual nanoseconds.
    ///
    /// [`RunOutcome::ReachedTime`] is returned only after CPU work at `target`
    /// completes and all events due at that instant, including zero-delay chains,
    /// have been dispatched. A target equal to the current machine time still
    /// enters the machine and drains that instant to establish this boundary.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::InvalidState`] unless the runtime is paused, or
    /// [`RuntimeError::TargetBeforeNow`] when `target` precedes the current machine
    /// time. A machine drive or event-dispatch failure returns
    /// [`RuntimeError::Machine`] and changes the runtime state to
    /// [`RuntimeState::Faulted`].
    pub fn run_until(&mut self, target: VTime) -> Result<RunOutcome, RuntimeError> {
        self.ensure_paused("run_until")?;
        let now = self.machine.now();
        if target < now {
            return Err(RuntimeError::TargetBeforeNow { now, target });
        }
        self.state = RuntimeState::Running;
        self.drive(Some(target))
    }

    /// Computes the machine's canonical guest-visible state digest.
    ///
    /// Runtime state and pending host control do not contribute to this value. A
    /// failure does not change the runtime state.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::Machine`] when the machine cannot encode or
    /// validate its canonical state.
    pub fn state_digest(&self) -> Result<StateDigest, RuntimeError> {
        self.machine.state_digest().map_err(RuntimeError::Machine)
    }

    /// Writes the current machine into the canonical `se_core` snapshot format.
    ///
    /// Runtime state and pending host control are omitted. The method accepts
    /// paused, halted, and faulted runtimes; a failure leaves the machine and
    /// runtime state unchanged but may leave partial bytes in `output`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::Snapshot`] for any stream, manifest, framing, or
    /// component-state failure reported by `se_core`.
    pub fn save_snapshot<W: Read + Write + Seek>(
        &self,
        output: &mut W,
    ) -> Result<(), RuntimeError> {
        write_snapshot(
            self.machine.as_ref(),
            self.build,
            self.factory.profile_fingerprint(),
            output,
        )
        .map_err(RuntimeError::Snapshot)
    }

    /// Loads and validates a snapshot into a fresh candidate machine.
    ///
    /// The current machine and runtime state remain unchanged on failure. After
    /// complete candidate validation succeeds, the candidate replaces the current
    /// machine and the runtime becomes [`RuntimeState::Paused`]; snapshot data does
    /// not restore prior runtime or host-control state.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::Snapshot`] for any compatibility, stream, framing,
    /// candidate-construction, component-state, integrity, or validation failure.
    pub fn load_snapshot<R: Read + Seek>(&mut self, input: &mut R) -> Result<(), RuntimeError> {
        let candidate = read_snapshot(input, self.build, self.factory.as_ref())
            .map_err(RuntimeError::Snapshot)?;
        self.machine = candidate;
        self.host_pause_requests = 0;
        self.state = RuntimeState::Paused;
        Ok(())
    }

    fn begin_run(&mut self, operation: &'static str) -> Result<(), RuntimeError> {
        self.ensure_paused(operation)?;
        self.state = RuntimeState::Running;
        Ok(())
    }

    fn ensure_paused(&self, operation: &'static str) -> Result<(), RuntimeError> {
        if self.state != RuntimeState::Paused {
            return Err(RuntimeError::InvalidState {
                operation,
                state: self.state,
            });
        }
        Ok(())
    }

    fn drive(&mut self, target: Option<VTime>) -> Result<RunOutcome, RuntimeError> {
        loop {
            let event_horizon = self.machine.front_event_time().unwrap_or(NO_DEADLINE);
            let deadline = target.map_or(event_horizon, |limit| event_horizon.min(limit));
            let exit = match self.machine.run_cpu_until(deadline) {
                Ok(exit) => exit,
                Err(error) => return self.machine_failed(error),
            };

            match exit {
                CpuExit::Deadline => {
                    if let Err(error) = self.drain_due_events() {
                        return self.machine_failed(error);
                    }
                    if target == Some(deadline) {
                        self.state = RuntimeState::Paused;
                        return Ok(RunOutcome::ReachedTime(deadline));
                    }
                }
                CpuExit::Reschedule => {}
                CpuExit::HostWake => {
                    if self.drain_host_work() {
                        self.state = RuntimeState::Paused;
                        return Ok(RunOutcome::Paused(PauseReason::HostRequest));
                    }
                }
                CpuExit::Breakpoint => {
                    self.state = RuntimeState::Paused;
                    return Ok(RunOutcome::Paused(PauseReason::Breakpoint));
                }
                CpuExit::Halted => {
                    self.state = RuntimeState::Halted;
                    return Ok(RunOutcome::Halted);
                }
            }
        }
    }

    fn drain_due_events(&mut self) -> Result<(), MachineError> {
        while let Some(event) = self.machine.pop_event()? {
            self.machine.dispatch_event(event)?;
        }
        Ok(())
    }

    fn drain_host_work(&mut self) -> bool {
        std::mem::take(&mut self.host_pause_requests) != 0
    }

    fn machine_failed<T>(&mut self, error: MachineError) -> Result<T, RuntimeError> {
        self.state = RuntimeState::Faulted;
        Err(RuntimeError::Machine(error))
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write;

    use se_core::event::ScheduledEvent;
    use se_core::inspect::{InspectCommand, InspectError, Introspect};
    use se_core::machine::{
        CpuExit, Machine, MachineCreateError, MachineError, MachineFactory, StateDigest,
    };
    use se_core::save::{StateError, StateReader, StateWriter};
    use se_core::snapshot::{
        BuildFingerprint, ComponentKey, ProfileFingerprint, SnapshotComponent, SnapshotTarget,
    };
    use se_core::time::VTime;

    use crate::{PauseReason, RunOutcome, Runtime, RuntimeState};

    const BUILD: BuildFingerprint = BuildFingerprint::from_bytes([0x55; 32]);
    const PROFILE: ProfileFingerprint = ProfileFingerprint::from_bytes([0x66; 32]);

    struct HostWakeMachine;

    impl SnapshotTarget for HostWakeMachine {
        fn snapshot_components(&self) -> &[SnapshotComponent] {
            &[]
        }

        fn save_component(
            &self,
            key: &ComponentKey,
            _writer: &mut StateWriter<'_>,
        ) -> Result<(), StateError> {
            Err(StateError::UnknownComponent(key.to_string()))
        }

        fn load_component(
            &mut self,
            key: &ComponentKey,
            _version: u32,
            _reader: &mut StateReader<'_>,
        ) -> Result<(), StateError> {
            Err(StateError::UnknownComponent(key.to_string()))
        }

        fn validate_loaded_snapshot(&self) -> Result<(), StateError> {
            Ok(())
        }
    }

    impl Introspect for HostWakeMachine {
        fn commands(&self) -> &[InspectCommand] {
            &[]
        }

        fn execute(
            &mut self,
            command: &str,
            _arguments: &[&str],
            _output: &mut dyn Write,
        ) -> Result<(), InspectError> {
            Err(InspectError::UnknownCommand(command.to_owned()))
        }
    }

    impl Machine for HostWakeMachine {
        fn now(&self) -> VTime {
            0
        }

        fn front_event_time(&mut self) -> Option<VTime> {
            None
        }

        fn run_cpu_until(&mut self, _deadline: VTime) -> Result<CpuExit, MachineError> {
            Ok(CpuExit::HostWake)
        }

        fn pop_event(&mut self) -> Result<Option<ScheduledEvent>, MachineError> {
            Ok(None)
        }

        fn dispatch_event(&mut self, _event: ScheduledEvent) -> Result<(), MachineError> {
            Ok(())
        }

        fn state_digest(&self) -> Result<StateDigest, MachineError> {
            Ok(StateDigest::from_bytes([0; 32]))
        }
    }

    struct HostWakeFactory;

    impl MachineFactory for HostWakeFactory {
        fn profile_fingerprint(&self) -> ProfileFingerprint {
            PROFILE
        }

        fn create(&self) -> Result<Box<dyn Machine>, MachineCreateError> {
            Ok(Box::new(HostWakeMachine))
        }
    }

    #[test]
    fn host_wake_merges_pause_requests_into_persistent_paused_state() {
        let mut runtime = Runtime::new(Box::new(HostWakeFactory), BUILD).unwrap();
        let digest = runtime.state_digest().unwrap();
        runtime.host_pause_requests = 2;

        assert_eq!(
            runtime.run().unwrap(),
            RunOutcome::Paused(PauseReason::HostRequest)
        );
        assert_eq!(runtime.state(), RuntimeState::Paused);
        assert_eq!(runtime.host_pause_requests, 0);
        assert_eq!(runtime.state_digest().unwrap(), digest);
    }
}
