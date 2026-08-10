use std::fmt::Write;

use serde::{Deserialize, Serialize};

use se_core::device::DeviceId;
use se_core::event::{ScheduleToken, ScheduledEvent, SchedulerShared};
use se_core::inspect::{InspectCommand, InspectError, Introspect};
use se_core::interrupt::{
    EVENT_TRUNCATE, GUEST_INTERRUPT_MASK, HOST_WAKE, InterruptSink, InterruptWord, WordLineSink,
};
use se_core::machine::{
    CpuExit, Machine, MachineCreateError, MachineError, MachineFactory, StateDigest,
};
use se_core::save::{Saveable, StateError, StateReader, StateWriter};
use se_core::snapshot::{
    BuildFingerprint, ComponentKey, ProfileFingerprint, SnapshotComponent, SnapshotTarget,
    decode_snapshot, encode_snapshot,
};
use se_core::time::VTime;

const BUILD: BuildFingerprint = BuildFingerprint::from_bytes([0x61; 32]);
const PROFILE: ProfileFingerprint = ProfileFingerprint::from_bytes([0x72; 32]);
const DEVICE: DeviceId = DeviceId::from_raw(0);
const INSTRUCTION_NS: VTime = 10;
const PERIODIC_TAG: u32 = 1;
const INTERRUPT_TAG: u32 = 2;
const INTERRUPT_LINE: u8 = 4;
const INTERRUPT_MASK: u64 = 1 << INTERRUPT_LINE;

#[derive(Serialize, Deserialize)]
struct CpuState {
    retired: u64,
    next_boundary: VTime,
    accumulator: u64,
    interrupt_seen_at: Option<u64>,
    schedule_at_retired: Option<u64>,
}

struct MockCpu {
    retired: u64,
    next_boundary: VTime,
    accumulator: u64,
    interrupt_seen_at: Option<VTime>,
    interrupt_word: InterruptWord,
    schedule_at_retired: Option<u64>,
}

#[derive(Serialize, Deserialize)]
struct DeviceState {
    dispatch_count: u64,
    last_event_time: Option<u64>,
    token: Option<u64>,
    interrupt_asserted: bool,
}

struct MockDevice {
    dispatch_count: u64,
    last_event_time: Option<VTime>,
    token: Option<ScheduleToken>,
    interrupt_asserted: bool,
    interrupt_sink: WordLineSink,
}

struct DeterministicMachine {
    manifest: Vec<SnapshotComponent>,
    scheduler: SchedulerShared,
    cpu: MockCpu,
    device: MockDevice,
}

impl DeterministicMachine {
    fn new() -> Self {
        let interrupt_word = InterruptWord::new();
        let interrupt_sink = WordLineSink::new(interrupt_word.clone(), INTERRUPT_LINE).unwrap();
        Self {
            manifest: vec![
                SnapshotComponent {
                    key: ComponentKey::new("core/event-queue").unwrap(),
                    schema_version: 1,
                    max_payload_len: 1 << 20,
                },
                SnapshotComponent {
                    key: ComponentKey::new("cpu/0").unwrap(),
                    schema_version: 1,
                    max_payload_len: 128,
                },
                SnapshotComponent {
                    key: ComponentKey::new("device/mock/0").unwrap(),
                    schema_version: 1,
                    max_payload_len: 64,
                },
            ],
            scheduler: SchedulerShared::new(),
            cpu: MockCpu {
                retired: 0,
                next_boundary: INSTRUCTION_NS,
                accumulator: 0x1234_5678_9abc_def0,
                interrupt_seen_at: None,
                interrupt_word,
                schedule_at_retired: None,
            },
            device: MockDevice {
                dispatch_count: 0,
                last_event_time: None,
                token: None,
                interrupt_asserted: false,
                interrupt_sink,
            },
        }
    }

    fn schedule_at(&mut self, vtime: VTime, tag: u32, payload: u64) {
        let token = self
            .scheduler
            .handle(DEVICE)
            .schedule_at(vtime, tag, payload)
            .unwrap();
        self.device.token = Some(token);
    }

    fn cpu_state(&self) -> CpuState {
        CpuState {
            retired: self.cpu.retired,
            next_boundary: self.cpu.next_boundary,
            accumulator: self.cpu.accumulator,
            interrupt_seen_at: self.cpu.interrupt_seen_at,
            schedule_at_retired: self.cpu.schedule_at_retired,
        }
    }

    fn device_state(&self) -> DeviceState {
        DeviceState {
            dispatch_count: self.device.dispatch_count,
            last_event_time: self.device.last_event_time,
            token: self.device.token.map(ScheduleToken::to_raw),
            interrupt_asserted: self.device.interrupt_asserted,
        }
    }

    fn drive_interrupt_line(&mut self, asserted: bool) {
        self.device.interrupt_asserted = asserted;
        self.device.interrupt_sink.set(asserted);
    }
}

impl SnapshotTarget for DeterministicMachine {
    fn snapshot_components(&self) -> &[SnapshotComponent] {
        &self.manifest
    }

    fn save_component(
        &self,
        key: &ComponentKey,
        writer: &mut StateWriter<'_>,
    ) -> Result<(), StateError> {
        match key.as_str() {
            "core/event-queue" => self.scheduler.save(writer),
            "cpu/0" => writer.serialize(&self.cpu_state()),
            "device/mock/0" => writer.serialize(&self.device_state()),
            _ => Err(StateError::UnknownComponent(key.to_string())),
        }
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
        match key.as_str() {
            "core/event-queue" => self.scheduler.load(version, reader),
            "cpu/0" => {
                let state: CpuState = reader.deserialize()?;
                let expected_next_boundary = state
                    .retired
                    .checked_add(1)
                    .and_then(|boundary| boundary.checked_mul(INSTRUCTION_NS))
                    .ok_or_else(|| {
                        StateError::InvalidState("CPU clock phase overflows VTime".to_owned())
                    })?;
                if state.next_boundary != expected_next_boundary {
                    return Err(StateError::InvalidState(
                        "CPU snapshot contains an inconsistent clock phase".to_owned(),
                    ));
                }
                self.cpu.retired = state.retired;
                self.cpu.next_boundary = state.next_boundary;
                self.cpu.accumulator = state.accumulator;
                self.cpu.interrupt_seen_at = state.interrupt_seen_at;
                self.cpu.schedule_at_retired = state.schedule_at_retired;
                Ok(())
            }
            "device/mock/0" => {
                let state: DeviceState = reader.deserialize()?;
                self.device.dispatch_count = state.dispatch_count;
                self.device.last_event_time = state.last_event_time;
                self.device.token = state.token.map(ScheduleToken::from_raw);
                self.drive_interrupt_line(state.interrupt_asserted);
                Ok(())
            }
            _ => Err(StateError::UnknownComponent(key.to_string())),
        }
    }

    fn validate_loaded_snapshot(&self) -> Result<(), StateError> {
        self.scheduler.validate_device_ids(1)?;
        let previous_boundary = self
            .cpu
            .next_boundary
            .checked_sub(INSTRUCTION_NS)
            .ok_or_else(|| {
                StateError::InvalidState("CPU clock phase precedes its epoch".to_owned())
            })?;
        if !(previous_boundary..self.cpu.next_boundary).contains(&self.now()) {
            return Err(StateError::InvalidState(
                "machine time lies outside the CPU clock phase".to_owned(),
            ));
        }
        if let Some(token) = self.device.token
            && !self.scheduler.handle(DEVICE).is_scheduled(token)
        {
            return Err(StateError::InvalidState(
                "device event token does not name a live event".to_owned(),
            ));
        }
        let line_asserted = self.cpu.interrupt_word.load_relaxed() & INTERRUPT_MASK != 0;
        if line_asserted != self.device.interrupt_asserted {
            return Err(StateError::InvalidState(
                "device interrupt state does not match its output line".to_owned(),
            ));
        }
        Ok(())
    }
}

impl Introspect for DeterministicMachine {
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

impl Machine for DeterministicMachine {
    fn now(&self) -> VTime {
        self.scheduler.now()
    }

    fn front_event_time(&mut self) -> Option<VTime> {
        self.scheduler.front_time()
    }

    fn run_cpu_until(&mut self, deadline: VTime) -> Result<CpuExit, MachineError> {
        if deadline < self.now() {
            return Err(MachineError::Failed(
                "CPU deadline precedes machine time".to_owned(),
            ));
        }
        let _burst = self
            .scheduler
            .begin_burst(deadline, self.cpu.interrupt_word.clone())
            .map_err(|error| MachineError::Failed(error.to_string()))?;
        loop {
            if self.cpu.next_boundary <= self.now() {
                return Err(MachineError::Failed(
                    "CPU clock phase does not follow machine time".to_owned(),
                ));
            }
            let previous_boundary = self.cpu.next_boundary - INSTRUCTION_NS;
            let pending = self.cpu.interrupt_word.load_relaxed();
            if pending & HOST_WAKE != 0 {
                let consumed = self.cpu.interrupt_word.take_host_wake();
                debug_assert!(consumed);
                return Ok(CpuExit::HostWake);
            }
            if pending & EVENT_TRUNCATE != 0 {
                return Ok(CpuExit::Reschedule);
            }
            if self.now() == deadline {
                return Ok(CpuExit::Deadline);
            }
            if self.now() == previous_boundary
                && pending & GUEST_INTERRUPT_MASK != 0
                && self.cpu.interrupt_seen_at.is_none()
            {
                self.cpu.interrupt_seen_at = Some(self.now());
            }
            let completion = self.cpu.next_boundary;
            if completion > deadline {
                self.scheduler.advance_to(deadline)?;
                return Ok(CpuExit::Deadline);
            }
            let next_boundary = completion
                .checked_add(INSTRUCTION_NS)
                .ok_or_else(|| MachineError::Failed("instruction time overflow".to_owned()))?;
            let retired = self
                .cpu
                .retired
                .checked_add(1)
                .ok_or_else(|| MachineError::Failed("retired count overflow".to_owned()))?;
            self.scheduler.advance_to(completion)?;
            self.cpu.next_boundary = next_boundary;
            self.cpu.retired = retired;
            self.cpu.accumulator = self
                .cpu
                .accumulator
                .rotate_left(7)
                .wrapping_add(self.cpu.retired ^ completion);
            if self.cpu.schedule_at_retired == Some(self.cpu.retired) {
                let token = self.scheduler.handle(DEVICE).schedule_after(
                    INSTRUCTION_NS,
                    PERIODIC_TAG,
                    0,
                )?;
                self.device.token = Some(token);
                self.cpu.schedule_at_retired = None;
            }
        }
    }

    fn pop_event(&mut self) -> Result<Option<ScheduledEvent>, MachineError> {
        Ok(self.scheduler.pop_due()?)
    }

    fn dispatch_event(&mut self, event: ScheduledEvent) -> Result<(), MachineError> {
        if event.device != DEVICE || event.vtime > self.now() {
            return Err(MachineError::Failed(
                "invalid mock event delivery".to_owned(),
            ));
        }
        self.device.dispatch_count += 1;
        self.device.last_event_time = Some(self.now());
        self.device.token = None;
        self.cpu.accumulator ^= event.payload.wrapping_mul(0x9e37_79b9);
        match event.tag {
            PERIODIC_TAG if event.payload != 0 => {
                let token = self.scheduler.handle(DEVICE).schedule_after(
                    17,
                    PERIODIC_TAG,
                    event.payload - 1,
                )?;
                self.device.token = Some(token);
            }
            PERIODIC_TAG => {}
            INTERRUPT_TAG => self.drive_interrupt_line(true),
            _ => return Err(MachineError::Failed("unknown mock event tag".to_owned())),
        }
        Ok(())
    }

    fn state_digest(&self) -> Result<StateDigest, MachineError> {
        let mut event_payload = Vec::new();
        let mut event_writer = StateWriter::new(&mut event_payload);
        self.scheduler.save(&mut event_writer)?;
        event_writer.finish()?;

        let cpu = postcard::to_stdvec(&self.cpu_state()).map_err(StateError::Encode)?;
        let device = postcard::to_stdvec(&self.device_state()).map_err(StateError::Encode)?;
        let mut hasher = blake3::Hasher::new();
        for bytes in [&event_payload, &cpu, &device] {
            hasher.update(&(bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
        }
        Ok(StateDigest::from_bytes(*hasher.finalize().as_bytes()))
    }
}

struct DeterministicFactory;

impl MachineFactory for DeterministicFactory {
    fn profile_fingerprint(&self) -> ProfileFingerprint {
        PROFILE
    }

    fn create(&self) -> Result<Box<dyn Machine>, MachineCreateError> {
        Ok(Box::new(DeterministicMachine::new()))
    }
}

fn drive_until(machine: &mut dyn Machine, target: VTime) {
    while machine.now() < target {
        let deadline = machine.front_event_time().unwrap_or(target).min(target);
        machine.run_cpu_until(deadline).unwrap();
        while let Some(event) = machine.pop_event().unwrap() {
            machine.dispatch_event(event).unwrap();
        }
    }
}

#[test]
fn event_dispatch_occurs_at_exact_virtual_time() {
    let mut machine = DeterministicMachine::new();
    machine.schedule_at(25, PERIODIC_TAG, 0);
    drive_until(&mut machine, 25);
    assert_eq!(machine.device.dispatch_count, 1);
    assert_eq!(machine.device.last_event_time, Some(25));
}

#[test]
fn off_grid_events_preserve_absolute_cpu_phase() {
    let mut machine = DeterministicMachine::new();
    machine.schedule_at(25, PERIODIC_TAG, 0);
    drive_until(&mut machine, 25);
    assert_eq!(machine.cpu.retired, 2);
    assert_eq!(machine.cpu.next_boundary, 30);

    machine.schedule_at(27, PERIODIC_TAG, 0);
    drive_until(&mut machine, 27);
    assert_eq!(machine.cpu.retired, 2);
    assert_eq!(machine.cpu.next_boundary, 30);

    assert_eq!(machine.run_cpu_until(30).unwrap(), CpuExit::Deadline);
    assert_eq!(machine.cpu.retired, 3);
    assert_eq!(machine.cpu.next_boundary, 40);
}

#[test]
fn earlier_event_scheduled_inside_burst_requests_truncation() {
    let mut machine = DeterministicMachine::new();
    machine.cpu.schedule_at_retired = Some(2);
    assert_eq!(machine.run_cpu_until(100).unwrap(), CpuExit::Reschedule);
    assert_eq!(machine.now(), 20);
    assert_eq!(machine.front_event_time(), Some(30));
    assert_eq!(
        machine.cpu.interrupt_word.load_relaxed() & EVENT_TRUNCATE,
        0
    );
}

#[test]
fn guest_interrupt_is_handled_inside_cpu_at_exact_instruction_boundary() {
    let mut machine = DeterministicMachine::new();
    machine.schedule_at(30, INTERRUPT_TAG, 0);
    assert_eq!(machine.run_cpu_until(30).unwrap(), CpuExit::Deadline);
    let event = machine.pop_event().unwrap().unwrap();
    machine.dispatch_event(event).unwrap();
    assert_eq!(machine.run_cpu_until(100).unwrap(), CpuExit::Deadline);
    assert_eq!(machine.cpu.interrupt_seen_at, Some(30));
    assert_eq!(machine.cpu.retired, 10);
    assert_eq!(
        machine.cpu.interrupt_word.load_relaxed() & INTERRUPT_MASK,
        INTERRUPT_MASK
    );
}

#[test]
fn equal_time_events_are_drained_before_guest_interrupt_sampling() {
    let mut machine = DeterministicMachine::new();
    machine.schedule_at(30, INTERRUPT_TAG, 0);
    machine.schedule_at(30, PERIODIC_TAG, 0);
    drive_until(&mut machine, 30);
    assert_eq!(machine.device.dispatch_count, 2);
    assert_eq!(machine.cpu.interrupt_seen_at, None);
    assert_eq!(machine.cpu.retired, 3);

    assert_eq!(machine.run_cpu_until(100).unwrap(), CpuExit::Deadline);
    assert_eq!(machine.cpu.interrupt_seen_at, Some(30));
    assert_eq!(machine.cpu.retired, 10);
}

#[test]
fn off_grid_guest_interrupt_waits_for_the_next_instruction_boundary() {
    let mut machine = DeterministicMachine::new();
    machine.schedule_at(25, INTERRUPT_TAG, 0);
    drive_until(&mut machine, 25);
    assert_eq!(machine.cpu.interrupt_seen_at, None);
    assert_eq!(machine.cpu.retired, 2);

    assert_eq!(machine.run_cpu_until(100).unwrap(), CpuExit::Deadline);
    assert_eq!(machine.cpu.interrupt_seen_at, Some(30));
    assert_eq!(machine.cpu.retired, 10);
    assert_eq!(machine.cpu.next_boundary, 110);
}

#[test]
fn host_wake_exits_without_advancing_guest_state() {
    let mut machine = DeterministicMachine::new();
    let wake = machine.cpu.interrupt_word.host_wake_handle();
    machine.drive_interrupt_line(true);
    wake.request();

    assert_eq!(machine.run_cpu_until(100).unwrap(), CpuExit::HostWake);
    assert_eq!(machine.now(), 0);
    assert_eq!(machine.cpu.retired, 0);
    assert_eq!(machine.cpu.interrupt_word.load_relaxed() & HOST_WAKE, 0);
    assert_eq!(
        machine.cpu.interrupt_word.load_relaxed() & INTERRUPT_MASK,
        INTERRUPT_MASK
    );

    assert_eq!(machine.run_cpu_until(20).unwrap(), CpuExit::Deadline);
    assert_eq!(machine.cpu.interrupt_seen_at, Some(0));
    assert_eq!(machine.cpu.retired, 2);
    assert_eq!(
        machine.cpu.interrupt_word.load_relaxed() & INTERRUPT_MASK,
        INTERRUPT_MASK
    );
}

#[test]
fn host_wake_is_excluded_from_snapshot_and_state_digest() {
    let machine = DeterministicMachine::new();
    let baseline_snapshot = encode_snapshot(&machine, BUILD, PROFILE).unwrap();
    let baseline_digest = machine.state_digest().unwrap();

    machine.cpu.interrupt_word.host_wake_handle().request();

    assert_eq!(
        encode_snapshot(&machine, BUILD, PROFILE).unwrap(),
        baseline_snapshot
    );
    assert_eq!(machine.state_digest().unwrap(), baseline_digest);
    assert_eq!(
        machine.cpu.interrupt_word.load_relaxed() & HOST_WAKE,
        HOST_WAKE
    );
}

#[test]
fn device_owned_interrupt_level_round_trips_without_machine_exit() {
    let mut uninterrupted = DeterministicMachine::new();
    uninterrupted.drive_interrupt_line(true);
    let snapshot = encode_snapshot(&uninterrupted, BUILD, PROFILE).unwrap();

    let mut restored = decode_snapshot(&snapshot, BUILD, &DeterministicFactory).unwrap();
    assert_eq!(uninterrupted.run_cpu_until(20).unwrap(), CpuExit::Deadline);
    assert_eq!(restored.run_cpu_until(20).unwrap(), CpuExit::Deadline);
    assert_eq!(
        uninterrupted.state_digest().unwrap(),
        restored.state_digest().unwrap()
    );
    assert_eq!(
        encode_snapshot(&uninterrupted, BUILD, PROFILE).unwrap(),
        encode_snapshot(restored.as_ref(), BUILD, PROFILE).unwrap()
    );
}

#[test]
fn snapshot_resume_preserves_off_grid_cpu_phase() {
    let mut uninterrupted = DeterministicMachine::new();
    uninterrupted.schedule_at(25, PERIODIC_TAG, 0);
    drive_until(&mut uninterrupted, 25);
    let snapshot = encode_snapshot(&uninterrupted, BUILD, PROFILE).unwrap();

    let mut restored = decode_snapshot(&snapshot, BUILD, &DeterministicFactory).unwrap();
    assert_eq!(uninterrupted.run_cpu_until(40).unwrap(), CpuExit::Deadline);
    assert_eq!(restored.run_cpu_until(40).unwrap(), CpuExit::Deadline);
    assert_eq!(
        uninterrupted.state_digest().unwrap(),
        restored.state_digest().unwrap()
    );
}

#[test]
fn fresh_machine_snapshot_resume_matches_uninterrupted_digest() {
    let mut uninterrupted = DeterministicMachine::new();
    uninterrupted.schedule_at(17, PERIODIC_TAG, 5);
    drive_until(&mut uninterrupted, 83);
    let snapshot = encode_snapshot(&uninterrupted, BUILD, PROFILE).unwrap();

    let mut restored = decode_snapshot(&snapshot, BUILD, &DeterministicFactory).unwrap();
    drive_until(&mut uninterrupted, 300);
    drive_until(restored.as_mut(), 300);
    assert_eq!(
        uninterrupted.state_digest().unwrap(),
        restored.state_digest().unwrap()
    );
    assert_eq!(
        encode_snapshot(&uninterrupted, BUILD, PROFILE).unwrap(),
        encode_snapshot(restored.as_ref(), BUILD, PROFILE).unwrap()
    );
}
