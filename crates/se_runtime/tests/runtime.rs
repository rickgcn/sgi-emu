use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::fmt::Write;
use std::io::Cursor;
use std::rc::Rc;

use se_core::device::DeviceId;
use se_core::event::ScheduledEvent;
use se_core::inspect::{InspectCommand, InspectError, Introspect};
use se_core::machine::{
    CpuExit, Machine, MachineCreateError, MachineError, MachineFactory, StateDigest,
};
use se_core::save::{StateError, StateReader, StateWriter};
use se_core::snapshot::{
    BuildFingerprint, ComponentKey, ProfileFingerprint, SnapshotComponent, SnapshotError,
    SnapshotTarget,
};
use se_core::time::{NO_DEADLINE, VTime};
use se_runtime::{PauseReason, RunOutcome, Runtime, RuntimeError, RuntimeState};

const BUILD: BuildFingerprint = BuildFingerprint::from_bytes([0x11; 32]);
const PROFILE: ProfileFingerprint = ProfileFingerprint::from_bytes([0x33; 32]);
const OTHER_PROFILE: ProfileFingerprint = ProfileFingerprint::from_bytes([0x44; 32]);
const DEVICE: DeviceId = DeviceId::from_raw(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StepExit {
    Deadline,
    Reschedule,
    HostWake,
    Breakpoint,
    Halted,
    Fail,
}

#[derive(Clone, Copy, Debug)]
struct RunStep {
    exit: StepExit,
    now: VTime,
    schedule: Option<ScheduledEvent>,
}

impl RunStep {
    const fn new(exit: StepExit, now: VTime) -> Self {
        Self {
            exit,
            now,
            schedule: None,
        }
    }

    const fn scheduling(exit: StepExit, now: VTime, event: ScheduledEvent) -> Self {
        Self {
            exit,
            now,
            schedule: Some(event),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum MachineCall {
    Front(Option<VTime>),
    Run(VTime),
    Pop,
    Dispatch(u32),
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum FactoryCall {
    Profile,
    Create(usize),
}

#[derive(Clone, Default)]
struct Blueprint {
    now: VTime,
    guest: u64,
    events: Vec<ScheduledEvent>,
    steps: Vec<RunStep>,
    chain: Option<(u32, ScheduledEvent)>,
    pop_fails: bool,
    dispatch_fail_tag: Option<u32>,
    digest_fails: bool,
}

struct FactoryControl {
    profile: Cell<ProfileFingerprint>,
    create_count: Cell<usize>,
    fail_on_create: Cell<Option<usize>>,
    alternate_manifest: Cell<bool>,
    validation_fails: Cell<bool>,
    calls: RefCell<Vec<FactoryCall>>,
    machine_calls: RefCell<Vec<Rc<RefCell<Vec<MachineCall>>>>>,
}

struct MockFactory {
    blueprint: Blueprint,
    control: Rc<FactoryControl>,
}

impl MockFactory {
    fn new(blueprint: Blueprint) -> (Self, Rc<FactoryControl>) {
        let control = Rc::new(FactoryControl {
            profile: Cell::new(PROFILE),
            create_count: Cell::new(0),
            fail_on_create: Cell::new(None),
            alternate_manifest: Cell::new(false),
            validation_fails: Cell::new(false),
            calls: RefCell::new(Vec::new()),
            machine_calls: RefCell::new(Vec::new()),
        });
        (
            Self {
                blueprint,
                control: Rc::clone(&control),
            },
            control,
        )
    }
}

impl MachineFactory for MockFactory {
    fn profile_fingerprint(&self) -> ProfileFingerprint {
        self.control.calls.borrow_mut().push(FactoryCall::Profile);
        self.control.profile.get()
    }

    fn create(&self) -> Result<Box<dyn Machine>, MachineCreateError> {
        let index = self.control.create_count.get();
        self.control.create_count.set(index + 1);
        self.control
            .calls
            .borrow_mut()
            .push(FactoryCall::Create(index));
        if self.control.fail_on_create.get() == Some(index) {
            return Err(MachineCreateError::new("mock creation failed"));
        }

        let calls = Rc::new(RefCell::new(Vec::new()));
        self.control
            .machine_calls
            .borrow_mut()
            .push(Rc::clone(&calls));
        Ok(Box::new(MockMachine::new(
            self.blueprint.clone(),
            Rc::clone(&self.control),
            calls,
        )))
    }
}

struct MockMachine {
    manifest: Vec<SnapshotComponent>,
    now: VTime,
    guest: u64,
    events: VecDeque<ScheduledEvent>,
    steps: VecDeque<RunStep>,
    chain: Option<(u32, ScheduledEvent)>,
    pop_fails: bool,
    dispatch_fail_tag: Option<u32>,
    digest_fails: bool,
    control: Rc<FactoryControl>,
    calls: Rc<RefCell<Vec<MachineCall>>>,
}

impl MockMachine {
    fn new(
        mut blueprint: Blueprint,
        control: Rc<FactoryControl>,
        calls: Rc<RefCell<Vec<MachineCall>>>,
    ) -> Self {
        blueprint.events.sort_by_key(|event| event.vtime);
        let key = if control.alternate_manifest.get() {
            "machine/alternate"
        } else {
            "machine/state"
        };
        Self {
            manifest: vec![SnapshotComponent {
                key: ComponentKey::new(key).unwrap(),
                schema_version: 1,
                max_payload_len: 4_096,
            }],
            now: blueprint.now,
            guest: blueprint.guest,
            events: blueprint.events.into(),
            steps: blueprint.steps.into(),
            chain: blueprint.chain,
            pop_fails: blueprint.pop_fails,
            dispatch_fail_tag: blueprint.dispatch_fail_tag,
            digest_fails: blueprint.digest_fails,
            control,
            calls,
        }
    }

    fn insert_event(&mut self, event: ScheduledEvent) {
        let index = self
            .events
            .iter()
            .position(|queued| queued.vtime > event.vtime)
            .unwrap_or(self.events.len());
        self.events.insert(index, event);
    }

    fn digest(&self) -> StateDigest {
        let mut bytes = [0_u8; 32];
        bytes[..8].copy_from_slice(&self.now.to_le_bytes());
        bytes[8..16].copy_from_slice(&self.guest.to_le_bytes());
        bytes[16..24].copy_from_slice(&(self.events.len() as u64).to_le_bytes());
        let event_fold = self.events.iter().fold(0_u64, |digest, event| {
            digest
                .wrapping_mul(0x100_0000_01b3)
                .wrapping_add(event.vtime)
                .wrapping_add(u64::from(event.tag))
                .wrapping_add(event.payload)
        });
        bytes[24..].copy_from_slice(&event_fold.to_le_bytes());
        StateDigest::from_bytes(bytes)
    }
}

impl SnapshotTarget for MockMachine {
    fn snapshot_components(&self) -> &[SnapshotComponent] {
        &self.manifest
    }

    fn save_component(
        &self,
        key: &ComponentKey,
        writer: &mut StateWriter<'_>,
    ) -> Result<(), StateError> {
        if key != &self.manifest[0].key {
            return Err(StateError::UnknownComponent(key.to_string()));
        }
        let events: Vec<_> = self
            .events
            .iter()
            .map(|event| (event.vtime, event.tag, event.payload))
            .collect();
        writer.serialize(&(self.now, self.guest, events))
    }

    fn load_component(
        &mut self,
        key: &ComponentKey,
        version: u32,
        reader: &mut StateReader<'_>,
    ) -> Result<(), StateError> {
        if version != 1 {
            return Err(StateError::UnsupportedVersion(version));
        }
        if key != &self.manifest[0].key {
            return Err(StateError::UnknownComponent(key.to_string()));
        }
        let (now, guest, events): (VTime, u64, Vec<(VTime, u32, u64)>) = reader.deserialize()?;
        self.now = now;
        self.guest = guest;
        self.events = events
            .into_iter()
            .map(|(vtime, tag, payload)| ScheduledEvent {
                vtime,
                device: DEVICE,
                tag,
                payload,
            })
            .collect();
        Ok(())
    }

    fn validate_loaded_snapshot(&self) -> Result<(), StateError> {
        if self.control.validation_fails.get() {
            return Err(StateError::InvalidState(
                "mock cross-component validation failed".to_owned(),
            ));
        }
        if self.events.iter().any(|event| event.vtime < self.now) {
            return Err(StateError::InvalidState(
                "mock event precedes machine time".to_owned(),
            ));
        }
        Ok(())
    }
}

impl Introspect for MockMachine {
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

impl Machine for MockMachine {
    fn now(&self) -> VTime {
        self.now
    }

    fn front_event_time(&mut self) -> Option<VTime> {
        let front = self.events.front().map(|event| event.vtime);
        self.calls.borrow_mut().push(MachineCall::Front(front));
        front
    }

    fn run_cpu_until(&mut self, deadline: VTime) -> Result<CpuExit, MachineError> {
        self.calls.borrow_mut().push(MachineCall::Run(deadline));
        let step = self
            .steps
            .pop_front()
            .unwrap_or_else(|| RunStep::new(StepExit::Deadline, deadline));
        if step.now < self.now || (deadline != NO_DEADLINE && step.now > deadline) {
            return Err(MachineError::Failed(
                "mock step violates its CPU deadline".to_owned(),
            ));
        }
        self.guest = self.guest.wrapping_add(step.now - self.now);
        self.now = step.now;
        if let Some(event) = step.schedule {
            self.insert_event(event);
        }
        match step.exit {
            StepExit::Deadline if self.now == deadline => Ok(CpuExit::Deadline),
            StepExit::Deadline => Err(MachineError::Failed(
                "mock deadline exit did not reach its deadline".to_owned(),
            )),
            StepExit::Reschedule => Ok(CpuExit::Reschedule),
            StepExit::HostWake => Ok(CpuExit::HostWake),
            StepExit::Breakpoint => Ok(CpuExit::Breakpoint),
            StepExit::Halted => Ok(CpuExit::Halted),
            StepExit::Fail => Err(MachineError::Failed("mock CPU failed".to_owned())),
        }
    }

    fn pop_event(&mut self) -> Result<Option<ScheduledEvent>, MachineError> {
        self.calls.borrow_mut().push(MachineCall::Pop);
        if self.pop_fails {
            return Err(MachineError::Failed("mock event pop failed".to_owned()));
        }
        if self
            .events
            .front()
            .is_some_and(|event| event.vtime <= self.now)
        {
            Ok(self.events.pop_front())
        } else {
            Ok(None)
        }
    }

    fn dispatch_event(&mut self, event: ScheduledEvent) -> Result<(), MachineError> {
        self.calls
            .borrow_mut()
            .push(MachineCall::Dispatch(event.tag));
        if self.dispatch_fail_tag == Some(event.tag) {
            return Err(MachineError::Failed(
                "mock event dispatch failed".to_owned(),
            ));
        }
        self.guest = self
            .guest
            .wrapping_add(u64::from(event.tag))
            .wrapping_add(event.payload);
        if let Some((source_tag, derived)) = self.chain
            && event.tag == source_tag
        {
            self.insert_event(derived);
        }
        Ok(())
    }

    fn state_digest(&self) -> Result<StateDigest, MachineError> {
        if self.digest_fails {
            return Err(MachineError::Failed("mock digest failed".to_owned()));
        }
        Ok(self.digest())
    }
}

fn event(vtime: VTime, tag: u32, payload: u64) -> ScheduledEvent {
    ScheduledEvent {
        vtime,
        device: DEVICE,
        tag,
        payload,
    }
}

fn runtime(blueprint: Blueprint) -> (Runtime, Rc<FactoryControl>) {
    let (factory, control) = MockFactory::new(blueprint);
    let runtime = Runtime::new(Box::new(factory), BUILD).unwrap();
    (runtime, control)
}

fn machine_calls(control: &FactoryControl, index: usize) -> Vec<MachineCall> {
    control.machine_calls.borrow()[index].borrow().clone()
}

fn run_deadlines(calls: &[MachineCall]) -> Vec<VTime> {
    calls
        .iter()
        .filter_map(|call| match call {
            MachineCall::Run(deadline) => Some(*deadline),
            _ => None,
        })
        .collect()
}

fn dispatched_tags(calls: &[MachineCall]) -> Vec<u32> {
    calls
        .iter()
        .filter_map(|call| match call {
            MachineCall::Dispatch(tag) => Some(*tag),
            _ => None,
        })
        .collect()
}

#[test]
fn construction_obtains_profile_before_creating_a_paused_machine() {
    let (runtime, control) = runtime(Blueprint::default());

    assert_eq!(runtime.state(), RuntimeState::Paused);
    assert_eq!(runtime.now(), 0);
    assert_eq!(
        control.calls.borrow().as_slice(),
        [FactoryCall::Profile, FactoryCall::Create(0)]
    );
    assert!(machine_calls(&control, 0).is_empty());
}

#[test]
fn construction_failure_is_reported_without_a_runtime() {
    let (factory, control) = MockFactory::new(Blueprint::default());
    control.fail_on_create.set(Some(0));

    let error = match Runtime::new(Box::new(factory), BUILD) {
        Ok(_) => panic!("mock construction unexpectedly succeeded"),
        Err(error) => error,
    };
    assert!(matches!(error, RuntimeError::MachineCreate(_)));
    assert_eq!(control.create_count.get(), 1);
}

#[test]
fn deadline_drains_same_time_fifo_chain_before_cpu_reentry() {
    let (mut runtime, control) = runtime(Blueprint {
        events: vec![event(25, 1, 0), event(25, 2, 0)],
        chain: Some((1, event(25, 3, 0))),
        ..Blueprint::default()
    });

    assert_eq!(runtime.run_until(25).unwrap(), RunOutcome::ReachedTime(25));
    assert_eq!(runtime.state(), RuntimeState::Paused);
    let calls = machine_calls(&control, 0);
    assert_eq!(run_deadlines(&calls), [25]);
    assert_eq!(dispatched_tags(&calls), [1, 2, 3]);
    assert_eq!(calls.last(), Some(&MachineCall::Pop));
}

#[test]
fn bounded_execution_uses_each_event_horizon_then_quiesces_target() {
    let (mut runtime, control) = runtime(Blueprint {
        events: vec![event(10, 1, 2), event(20, 2, 3)],
        ..Blueprint::default()
    });

    assert_eq!(runtime.run_until(30).unwrap(), RunOutcome::ReachedTime(30));
    let calls = machine_calls(&control, 0);
    assert_eq!(run_deadlines(&calls), [10, 20, 30]);
    assert_eq!(dispatched_tags(&calls), [1, 2]);
    assert_eq!(
        calls
            .iter()
            .filter(|call| matches!(call, MachineCall::Front(_)))
            .count(),
        3
    );
}

#[test]
fn target_equal_to_now_still_enters_cpu_and_drains_due_events() {
    let (mut runtime, control) = runtime(Blueprint {
        events: vec![event(0, 7, 0)],
        ..Blueprint::default()
    });

    assert_eq!(runtime.run_until(0).unwrap(), RunOutcome::ReachedTime(0));
    let calls = machine_calls(&control, 0);
    assert_eq!(run_deadlines(&calls), [0]);
    assert_eq!(dispatched_tags(&calls), [7]);
    assert_eq!(calls.last(), Some(&MachineCall::Pop));
}

#[test]
fn target_before_now_is_rejected_without_state_or_machine_changes() {
    let (mut runtime, control) = runtime(Blueprint {
        now: 10,
        guest: 5,
        ..Blueprint::default()
    });
    let digest = runtime.state_digest().unwrap();

    assert!(matches!(
        runtime.run_until(9),
        Err(RuntimeError::TargetBeforeNow { now: 10, target: 9 })
    ));
    assert_eq!(runtime.state(), RuntimeState::Paused);
    assert_eq!(runtime.state_digest().unwrap(), digest);
    assert!(machine_calls(&control, 0).is_empty());
}

#[test]
fn reschedule_discards_old_horizon_and_discovers_earlier_event() {
    let (mut runtime, control) = runtime(Blueprint {
        events: vec![event(150, 1, 0)],
        steps: vec![
            RunStep::scheduling(StepExit::Reschedule, 120, event(130, 2, 0)),
            RunStep::new(StepExit::Deadline, 130),
        ],
        ..Blueprint::default()
    });

    assert_eq!(
        runtime.run_until(160).unwrap(),
        RunOutcome::ReachedTime(160)
    );
    let calls = machine_calls(&control, 0);
    assert_eq!(run_deadlines(&calls), [150, 130, 150, 160]);
    assert_eq!(dispatched_tags(&calls), [2, 1]);
}

#[test]
fn host_wake_requeries_horizon_after_simultaneous_truncation() {
    let (mut runtime, control) = runtime(Blueprint {
        events: vec![event(150, 1, 0)],
        steps: vec![
            RunStep::scheduling(StepExit::HostWake, 120, event(130, 2, 0)),
            RunStep::new(StepExit::Deadline, 130),
        ],
        ..Blueprint::default()
    });

    assert_eq!(
        runtime.run_until(160).unwrap(),
        RunOutcome::ReachedTime(160)
    );
    let calls = machine_calls(&control, 0);
    assert_eq!(run_deadlines(&calls), [150, 130, 150, 160]);
    assert_eq!(dispatched_tags(&calls), [2, 1]);
}

#[test]
fn empty_host_wake_is_guest_neutral_and_requeries_before_breakpoint() {
    let (mut runtime, control) = runtime(Blueprint {
        steps: vec![
            RunStep::new(StepExit::HostWake, 0),
            RunStep::new(StepExit::Breakpoint, 0),
        ],
        ..Blueprint::default()
    });
    let before = runtime.state_digest().unwrap();

    assert_eq!(
        runtime.run().unwrap(),
        RunOutcome::Paused(PauseReason::Breakpoint)
    );
    assert_eq!(runtime.state_digest().unwrap(), before);
    let calls = machine_calls(&control, 0);
    assert_eq!(run_deadlines(&calls), [NO_DEADLINE, NO_DEADLINE]);
    assert_eq!(
        calls
            .iter()
            .filter(|call| matches!(call, MachineCall::Front(None)))
            .count(),
        2
    );
}

#[test]
fn unbounded_run_drains_deadline_then_returns_breakpoint() {
    let (mut runtime, control) = runtime(Blueprint {
        events: vec![event(10, 1, 0)],
        steps: vec![
            RunStep::new(StepExit::Deadline, 10),
            RunStep::new(StepExit::Breakpoint, 10),
        ],
        ..Blueprint::default()
    });

    assert_eq!(
        runtime.run().unwrap(),
        RunOutcome::Paused(PauseReason::Breakpoint)
    );
    let calls = machine_calls(&control, 0);
    assert_eq!(run_deadlines(&calls), [10, NO_DEADLINE]);
    assert_eq!(dispatched_tags(&calls), [1]);
}

#[test]
fn halted_runtime_rejects_normal_resume() {
    let (mut runtime, control) = runtime(Blueprint {
        steps: vec![RunStep::new(StepExit::Halted, 0)],
        ..Blueprint::default()
    });

    assert_eq!(runtime.run().unwrap(), RunOutcome::Halted);
    assert_eq!(runtime.state(), RuntimeState::Halted);
    assert!(matches!(
        runtime.run(),
        Err(RuntimeError::InvalidState {
            operation: "run",
            state: RuntimeState::Halted
        })
    ));
    assert_eq!(run_deadlines(&machine_calls(&control, 0)), [NO_DEADLINE]);
}

#[test]
fn cpu_pop_and_dispatch_failures_fault_the_runtime() {
    let scenarios = [
        Blueprint {
            steps: vec![RunStep::new(StepExit::Fail, 0)],
            ..Blueprint::default()
        },
        Blueprint {
            events: vec![event(10, 1, 0)],
            pop_fails: true,
            ..Blueprint::default()
        },
        Blueprint {
            events: vec![event(10, 1, 0)],
            dispatch_fail_tag: Some(1),
            ..Blueprint::default()
        },
    ];

    for blueprint in scenarios {
        let (mut runtime, _) = runtime(blueprint);
        let error = runtime.run_until(10).unwrap_err();
        assert!(matches!(error, RuntimeError::Machine(_)));
        assert_eq!(runtime.state(), RuntimeState::Faulted);
        assert!(matches!(
            runtime.run_until(20),
            Err(RuntimeError::InvalidState {
                operation: "run_until",
                state: RuntimeState::Faulted
            })
        ));
    }
}

#[test]
fn digest_failure_does_not_change_runtime_state() {
    let (runtime, _) = runtime(Blueprint {
        digest_fails: true,
        ..Blueprint::default()
    });

    assert!(matches!(
        runtime.state_digest(),
        Err(RuntimeError::Machine(_))
    ));
    assert_eq!(runtime.state(), RuntimeState::Paused);
}

#[test]
fn snapshot_header_uses_injected_build_and_factory_profile() {
    let (runtime, _) = runtime(Blueprint::default());
    let mut output = Cursor::new(Vec::new());

    runtime.save_snapshot(&mut output).unwrap();
    let bytes = output.into_inner();
    assert_eq!(&bytes[12..44], BUILD.as_bytes());
    assert_eq!(&bytes[44..76], PROFILE.as_bytes());
}

#[test]
fn successful_snapshot_load_replaces_halted_machine_and_pauses() {
    let (mut runtime, control) = runtime(Blueprint {
        guest: 7,
        steps: vec![
            RunStep::new(StepExit::Deadline, 10),
            RunStep::new(StepExit::Halted, 10),
        ],
        ..Blueprint::default()
    });
    assert_eq!(runtime.run_until(10).unwrap(), RunOutcome::ReachedTime(10));
    let saved_digest = runtime.state_digest().unwrap();
    let mut snapshot = Cursor::new(Vec::new());
    runtime.save_snapshot(&mut snapshot).unwrap();
    assert_eq!(runtime.run().unwrap(), RunOutcome::Halted);

    runtime.load_snapshot(&mut snapshot).unwrap();

    assert_eq!(runtime.state(), RuntimeState::Paused);
    assert_eq!(runtime.now(), 10);
    assert_eq!(runtime.state_digest().unwrap(), saved_digest);
    assert_eq!(control.create_count.get(), 2);
}

#[test]
fn snapshot_load_failures_preserve_current_machine_and_state() {
    let (mut runtime, control) = runtime(Blueprint {
        guest: 19,
        steps: vec![RunStep::new(StepExit::Halted, 0)],
        ..Blueprint::default()
    });
    let mut output = Cursor::new(Vec::new());
    runtime.save_snapshot(&mut output).unwrap();
    let bytes = output.into_inner();
    assert_eq!(runtime.run().unwrap(), RunOutcome::Halted);
    let old_digest = runtime.state_digest().unwrap();

    let mut wrong_build = bytes.clone();
    wrong_build[12] ^= 0xff;
    assert!(matches!(
        runtime.load_snapshot(&mut Cursor::new(wrong_build)),
        Err(RuntimeError::Snapshot(
            SnapshotError::BuildFingerprintMismatch
        ))
    ));
    assert_eq!(runtime.state(), RuntimeState::Halted);
    assert_eq!(runtime.state_digest().unwrap(), old_digest);

    control.profile.set(OTHER_PROFILE);
    assert!(matches!(
        runtime.load_snapshot(&mut Cursor::new(bytes.clone())),
        Err(RuntimeError::Snapshot(
            SnapshotError::ProfileFingerprintMismatch
        ))
    ));
    control.profile.set(PROFILE);
    assert_eq!(runtime.state(), RuntimeState::Halted);
    assert_eq!(runtime.state_digest().unwrap(), old_digest);

    control.alternate_manifest.set(true);
    assert!(matches!(
        runtime.load_snapshot(&mut Cursor::new(bytes.clone())),
        Err(RuntimeError::Snapshot(
            SnapshotError::ComponentKeyMismatch { .. }
        ))
    ));
    control.alternate_manifest.set(false);
    assert_eq!(runtime.state(), RuntimeState::Halted);
    assert_eq!(runtime.state_digest().unwrap(), old_digest);

    control.validation_fails.set(true);
    assert!(matches!(
        runtime.load_snapshot(&mut Cursor::new(bytes.clone())),
        Err(RuntimeError::Snapshot(SnapshotError::MachineValidation(_)))
    ));
    control.validation_fails.set(false);
    assert_eq!(runtime.state(), RuntimeState::Halted);
    assert_eq!(runtime.state_digest().unwrap(), old_digest);

    let mut corrupt = bytes;
    let payload_index = corrupt.len() - 33;
    corrupt[payload_index] ^= 0x80;
    assert!(matches!(
        runtime.load_snapshot(&mut Cursor::new(corrupt)),
        Err(RuntimeError::Snapshot(_))
    ));
    assert_eq!(runtime.state(), RuntimeState::Halted);
    assert_eq!(runtime.state_digest().unwrap(), old_digest);
}

#[test]
fn failed_candidate_creation_preserves_fault_then_valid_load_recovers() {
    let (mut runtime, control) = runtime(Blueprint {
        guest: 23,
        steps: vec![RunStep::new(StepExit::Fail, 0)],
        ..Blueprint::default()
    });
    let mut snapshot = Cursor::new(Vec::new());
    runtime.save_snapshot(&mut snapshot).unwrap();
    let saved_digest = runtime.state_digest().unwrap();
    assert!(matches!(runtime.run(), Err(RuntimeError::Machine(_))));
    assert_eq!(runtime.state(), RuntimeState::Faulted);

    control.fail_on_create.set(Some(1));
    assert!(matches!(
        runtime.load_snapshot(&mut snapshot),
        Err(RuntimeError::Snapshot(SnapshotError::MachineCreate(_)))
    ));
    assert_eq!(runtime.state(), RuntimeState::Faulted);
    assert_eq!(runtime.state_digest().unwrap(), saved_digest);

    control.fail_on_create.set(None);
    runtime.load_snapshot(&mut snapshot).unwrap();
    assert_eq!(runtime.state(), RuntimeState::Paused);
    assert_eq!(runtime.state_digest().unwrap(), saved_digest);
}

#[test]
fn snapshot_restore_resume_matches_uninterrupted_digest_and_bytes() {
    let blueprint = Blueprint {
        guest: 29,
        events: vec![event(17, 1, 5), event(200, 2, 7)],
        ..Blueprint::default()
    };
    let (mut uninterrupted, _) = runtime(blueprint.clone());
    let (mut restored, _) = runtime(blueprint);

    assert_eq!(
        uninterrupted.run_until(83).unwrap(),
        RunOutcome::ReachedTime(83)
    );
    let mut checkpoint = Cursor::new(Vec::new());
    uninterrupted.save_snapshot(&mut checkpoint).unwrap();
    restored.load_snapshot(&mut checkpoint).unwrap();

    assert_eq!(
        uninterrupted.run_until(300).unwrap(),
        RunOutcome::ReachedTime(300)
    );
    assert_eq!(
        restored.run_until(300).unwrap(),
        RunOutcome::ReachedTime(300)
    );
    assert_eq!(
        uninterrupted.state_digest().unwrap(),
        restored.state_digest().unwrap()
    );

    let mut uninterrupted_bytes = Cursor::new(Vec::new());
    let mut restored_bytes = Cursor::new(Vec::new());
    uninterrupted
        .save_snapshot(&mut uninterrupted_bytes)
        .unwrap();
    restored.save_snapshot(&mut restored_bytes).unwrap();
    assert_eq!(
        uninterrupted_bytes.into_inner(),
        restored_bytes.into_inner()
    );
}

#[test]
fn runtime_error_exposes_operation_context_and_sources() {
    use std::error::Error;

    let target = RuntimeError::TargetBeforeNow { now: 8, target: 7 };
    assert_eq!(
        target.to_string(),
        "run target 7 precedes current virtual time 8"
    );
    assert!(target.source().is_none());

    let machine = RuntimeError::Machine(MachineError::Failed("reason".to_owned()));
    assert!(machine.to_string().contains("reason"));
    assert!(machine.source().is_some());
}
