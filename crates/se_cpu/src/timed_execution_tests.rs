use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use se_core::address::PhysAddr;
use se_core::bus::{Bus, BusFault, BusInitiator, CpuId, DirectAccess, DirectSpan};
use se_core::device::DeviceId;
use se_core::event::{EventQueueError, SchedulerHandle, SchedulerShared};
use se_core::interrupt::{EVENT_TRUNCATE, GUEST_INTERRUPT_MASK, InterruptSink, WordLineSink};
use se_core::machine::CpuExit;
use se_core::time::VTime;

use crate::cp0::Cp0;
use crate::cpu::Cpu;
use crate::decode::Instruction;
use crate::exception::ExceptionCode;
use crate::execute::ExecuteError;
use crate::gpr::{GprFile, Reg};
use crate::memory::TranslationError;
use crate::pc::PcState;
use crate::run::{CpuRunContext, CpuRunError, TimedBusResult, TranslationAccess};
use crate::timing::{ProcessorClock, TimingError};

const CPU_ID: CpuId = CpuId::from_raw(7);
const TEST_DEVICE: DeviceId = DeviceId::from_raw(3);
const BOOT_VIRTUAL: u64 = 0xffff_ffff_bfc0_0000;
const BOOT_PHYSICAL: u64 = 0x1fc0_0000;
const DATA_VIRTUAL: u64 = 0xffff_ffff_a000_1000;
const DATA_PHYSICAL: u64 = 0x1000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TransactionKind {
    Read32,
    Write32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Transaction {
    kind: TransactionKind,
    initiator: BusInitiator,
    address: PhysAddr,
    time: VTime,
}

struct TestBus {
    scheduler: SchedulerShared,
    schedule_handle: SchedulerHandle,
    words: BTreeMap<u64, u32>,
    read_faults: BTreeMap<u64, BusFault>,
    write_faults: BTreeMap<u64, BusFault>,
    schedule_on_read: BTreeMap<u64, VTime>,
    schedule_on_write: BTreeMap<u64, VTime>,
    transactions: Vec<Transaction>,
}

impl TestBus {
    fn new(scheduler: SchedulerShared) -> Self {
        let schedule_handle = scheduler.handle(TEST_DEVICE);
        Self {
            scheduler,
            schedule_handle,
            words: BTreeMap::new(),
            read_faults: BTreeMap::new(),
            write_faults: BTreeMap::new(),
            schedule_on_read: BTreeMap::new(),
            schedule_on_write: BTreeMap::new(),
            transactions: Vec::new(),
        }
    }

    fn read32_for(
        &mut self,
        initiator: CpuId,
        time: VTime,
        address: PhysAddr,
    ) -> Result<u32, BusFault> {
        assert_eq!(self.scheduler.now(), time);
        self.transactions.push(Transaction {
            kind: TransactionKind::Read32,
            initiator: BusInitiator::Cpu(initiator),
            address,
            time,
        });
        self.read32(address)
    }

    fn write32_for(
        &mut self,
        initiator: CpuId,
        time: VTime,
        address: PhysAddr,
        value: u32,
    ) -> Result<(), BusFault> {
        assert_eq!(self.scheduler.now(), time);
        self.transactions.push(Transaction {
            kind: TransactionKind::Write32,
            initiator: BusInitiator::Cpu(initiator),
            address,
            time,
        });
        self.write32(address, value)
    }

    fn schedule_after_callback(&self, delay: VTime) {
        self.schedule_handle
            .schedule_after(delay, 1, 0)
            .expect("test device scheduling must succeed");
    }
}

impl Bus for TestBus {
    fn read8(&mut self, _address: PhysAddr) -> Result<u8, BusFault> {
        Err(BusFault::Fault)
    }

    fn read16(&mut self, _address: PhysAddr) -> Result<u16, BusFault> {
        Err(BusFault::Fault)
    }

    fn read32(&mut self, address: PhysAddr) -> Result<u32, BusFault> {
        if let Some(delay) = self.schedule_on_read.remove(&address.get()) {
            self.schedule_after_callback(delay);
        }
        if let Some(fault) = self.read_faults.get(&address.get()) {
            return Err(*fault);
        }
        self.words
            .get(&address.get())
            .copied()
            .ok_or(BusFault::Unmapped)
    }

    fn read64(&mut self, _address: PhysAddr) -> Result<u64, BusFault> {
        Err(BusFault::Fault)
    }

    fn write8(&mut self, _address: PhysAddr, _value: u8) -> Result<(), BusFault> {
        Err(BusFault::Fault)
    }

    fn write16(&mut self, _address: PhysAddr, _value: u16) -> Result<(), BusFault> {
        Err(BusFault::Fault)
    }

    fn write32(&mut self, address: PhysAddr, value: u32) -> Result<(), BusFault> {
        // The write is intentionally visible before a configured mapped-device
        // fault, matching the core Bus contract's lack of side-effect rollback.
        self.words.insert(address.get(), value);
        if let Some(delay) = self.schedule_on_write.remove(&address.get()) {
            self.schedule_after_callback(delay);
        }
        if let Some(fault) = self.write_faults.get(&address.get()) {
            return Err(*fault);
        }
        Ok(())
    }

    fn write64(&mut self, _address: PhysAddr, _value: u64) -> Result<(), BusFault> {
        Err(BusFault::Fault)
    }

    fn direct_span(
        &mut self,
        _address: PhysAddr,
        _requested: usize,
        _access: DirectAccess,
    ) -> Result<Option<DirectSpan<'_>>, BusFault> {
        Ok(None)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TestContextError {
    Scheduler(EventQueueError),
    Injected,
}

impl fmt::Display for TestContextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scheduler(error) => write!(formatter, "test scheduler failure: {error}"),
            Self::Injected => formatter.write_str("injected context failure"),
        }
    }
}

impl Error for TestContextError {}

impl From<EventQueueError> for TestContextError {
    fn from(error: EventQueueError) -> Self {
        Self::Scheduler(error)
    }
}

struct TestContext<'a> {
    scheduler: &'a SchedulerShared,
    bus: &'a mut TestBus,
    cpu_id: CpuId,
    fail_synchronization: bool,
}

impl CpuRunContext for TestContext<'_> {
    type Error = TestContextError;

    fn now(&self) -> VTime {
        self.scheduler.now()
    }

    fn synchronize_to(&mut self, time: VTime) -> Result<(), Self::Error> {
        if self.fail_synchronization {
            return Err(TestContextError::Injected);
        }
        self.scheduler.advance_to(time).map_err(Into::into)
    }

    fn read32_at(&mut self, time: VTime, address: PhysAddr) -> TimedBusResult<u32, Self::Error> {
        self.synchronize_to(time)?;
        Ok(self.bus.read32_for(self.cpu_id, time, address))
    }

    fn write32_at(
        &mut self,
        time: VTime,
        address: PhysAddr,
        value: u32,
    ) -> TimedBusResult<(), Self::Error> {
        self.synchronize_to(time)?;
        Ok(self.bus.write32_for(self.cpu_id, time, address, value))
    }
}

struct TestMachine {
    cpu: Cpu,
    scheduler: SchedulerShared,
    bus: TestBus,
    cpu_id: CpuId,
}

impl TestMachine {
    fn new(frequency_hz: u64, entry_pc: u64, initial_gprs: &[(Reg, u64)]) -> Self {
        let scheduler = SchedulerShared::new();
        let mut gpr = GprFile::new();
        for &(register, value) in initial_gprs {
            gpr.write(register, value);
        }
        let cpu = Cpu::from_parts(
            gpr,
            PcState::new(entry_pc),
            Cp0::synthetic_test_state(false),
            ProcessorClock::new(frequency_hz).expect("test PClk must be representable"),
        );
        let bus = TestBus::new(scheduler.clone());
        Self {
            cpu,
            scheduler,
            bus,
            cpu_id: CPU_ID,
        }
    }

    fn run_until(&mut self, deadline: VTime) -> Result<CpuExit, CpuRunError<TestContextError>> {
        let Self {
            cpu,
            scheduler,
            bus,
            cpu_id,
        } = self;
        let _burst = scheduler
            .begin_burst(deadline, cpu.interrupt_word().clone())
            .expect("test burst horizon must be valid");
        let mut context = TestContext {
            scheduler,
            bus,
            cpu_id: *cpu_id,
            fail_synchronization: false,
        };
        cpu.run_until(&mut context, deadline)
    }

    fn install_program(&mut self, instructions: &[u32]) {
        for (index, instruction) in instructions.iter().copied().enumerate() {
            let offset = u64::try_from(index).unwrap() * 4;
            self.bus.words.insert(BOOT_PHYSICAL + offset, instruction);
        }
    }
}

fn reg(index: u8) -> Reg {
    Reg::new(index).expect("test register index must be architectural")
}

fn encode_i(opcode: u8, base: u8, rt: u8, immediate: u16) -> u32 {
    (u32::from(opcode) << 26)
        | (u32::from(base) << 21)
        | (u32::from(rt) << 16)
        | u32::from(immediate)
}

fn assert_exception(machine: &TestMachine, code: ExceptionCode, bad_vaddr: u64) {
    assert_eq!(machine.cpu.cp0().exception_code(), code);
    assert_eq!(machine.cpu.cp0().bad_vaddr(), bad_vaddr);
    assert_eq!(machine.cpu.pc_state().current(), 0xffff_ffff_8000_0180);
}

#[test]
fn bounded_180_mhz_execution_preserves_the_fourth_absolute_phase() {
    let mut machine = TestMachine::new(180_000_000, BOOT_VIRTUAL, &[]);
    machine.install_program(&[
        encode_i(0x0f, 0, 1, 0x1234),
        encode_i(0x0d, 1, 1, 0x5678),
        encode_i(0x09, 0, 2, 7),
        encode_i(0x09, 0, 3, 9),
    ]);

    assert_eq!(machine.run_until(20), Ok(CpuExit::Deadline));
    assert_eq!(machine.scheduler.now(), 20);
    assert_eq!(machine.cpu.next_pclk_tick(), 4);
    assert_eq!(machine.cpu.next_boundary(), Ok(23));
    assert_eq!(machine.cpu.pc_state().current(), BOOT_VIRTUAL + 12);
    assert_eq!(machine.cpu.read_gpr(reg(1)), 0x1234_5678);
    assert_eq!(machine.cpu.read_gpr(reg(2)), 7);
    assert_eq!(machine.cpu.read_gpr(reg(3)), 0);
    assert_eq!(
        machine.bus.transactions,
        [
            Transaction {
                kind: TransactionKind::Read32,
                initiator: BusInitiator::Cpu(CPU_ID),
                address: PhysAddr::new(BOOT_PHYSICAL),
                time: 6,
            },
            Transaction {
                kind: TransactionKind::Read32,
                initiator: BusInitiator::Cpu(CPU_ID),
                address: PhysAddr::new(BOOT_PHYSICAL + 4),
                time: 12,
            },
            Transaction {
                kind: TransactionKind::Read32,
                initiator: BusInitiator::Cpu(CPU_ID),
                address: PhysAddr::new(BOOT_PHYSICAL + 8),
                time: 17,
            },
        ]
    );
}

#[test]
fn equal_deadline_cpu_work_executes_before_deadline_exit() {
    let mut machine = TestMachine::new(180_000_000, BOOT_VIRTUAL, &[]);
    machine.install_program(&[
        encode_i(0x09, 0, 1, 1),
        encode_i(0x09, 0, 2, 2),
        encode_i(0x09, 0, 3, 3),
    ]);

    assert_eq!(machine.run_until(17), Ok(CpuExit::Deadline));
    assert_eq!(machine.scheduler.now(), 17);
    assert_eq!(machine.cpu.next_pclk_tick(), 4);
    assert_eq!(machine.cpu.read_gpr(reg(3)), 3);
    assert_eq!(machine.cpu.pc_state().current(), BOOT_VIRTUAL + 12);
}

#[test]
fn deadline_before_next_boundary_executes_nothing() {
    let mut machine = TestMachine::new(100_000_000, BOOT_VIRTUAL, &[]);
    machine.install_program(&[encode_i(0x09, 0, 1, 1)]);

    assert_eq!(machine.run_until(5), Ok(CpuExit::Deadline));
    assert_eq!(machine.scheduler.now(), 5);
    assert_eq!(machine.cpu.next_pclk_tick(), 1);
    assert_eq!(machine.cpu.next_boundary(), Ok(10));
    assert_eq!(machine.cpu.pc_state().current(), BOOT_VIRTUAL);
    assert!(machine.bus.transactions.is_empty());
}

#[test]
fn off_grid_deadline_does_not_rebase_cpu_phase() {
    let mut machine = TestMachine::new(180_000_000, BOOT_VIRTUAL, &[]);
    machine.install_program(&[0, 0, 0, 0]);
    assert_eq!(machine.run_until(17), Ok(CpuExit::Deadline));
    machine.scheduler.advance_to(18).unwrap();

    assert_eq!(machine.run_until(20), Ok(CpuExit::Deadline));
    assert_eq!(machine.scheduler.now(), 20);
    assert_eq!(machine.cpu.next_pclk_tick(), 4);
    assert_eq!(machine.cpu.next_boundary(), Ok(23));
    assert_eq!(machine.bus.transactions.len(), 3);
}

#[test]
fn phase_behind_machine_is_a_structured_timing_error() {
    let mut machine = TestMachine::new(100_000_000, BOOT_VIRTUAL, &[]);
    machine.install_program(&[0]);
    assert_eq!(machine.run_until(10), Ok(CpuExit::Deadline));
    machine.scheduler.advance_to(21).unwrap();

    assert_eq!(
        machine.run_until(30),
        Err(CpuRunError::Timing(TimingError::PhaseBehindMachine {
            next_boundary: 20,
            machine_now: 21,
        }))
    );
    assert_eq!(machine.cpu.next_pclk_tick(), 2);
}

#[test]
fn deadline_before_machine_time_is_rejected_without_rebasing_phase() {
    let mut machine = TestMachine::new(100_000_000, BOOT_VIRTUAL, &[]);
    machine.scheduler.advance_to(3).unwrap();
    let mut context = TestContext {
        scheduler: &machine.scheduler,
        bus: &mut machine.bus,
        cpu_id: machine.cpu_id,
        fail_synchronization: false,
    };

    assert_eq!(
        machine.cpu.run_until(&mut context, 2),
        Err(CpuRunError::Timing(TimingError::DeadlineBeforeMachine {
            deadline: 2,
            machine_now: 3,
        }))
    );
    assert_eq!(machine.cpu.next_pclk_tick(), 1);
    assert!(context.bus.transactions.is_empty());
}

#[test]
fn method_entry_host_wake_preserves_off_grid_time_phase_and_architecture() {
    let mut machine = TestMachine::new(100_000_000, BOOT_VIRTUAL, &[]);
    machine.install_program(&[encode_i(0x09, 0, 1, 1)]);
    machine.scheduler.advance_to(3).unwrap();
    machine.cpu.interrupt_word().host_wake_handle().request();
    let before = machine.cpu.clone();

    assert_eq!(machine.run_until(100), Ok(CpuExit::HostWake));
    assert_eq!(machine.scheduler.now(), 3);
    assert_eq!(machine.cpu, before);
    assert_eq!(machine.cpu.next_pclk_tick(), 1);
    assert!(machine.bus.transactions.is_empty());
}

#[test]
fn host_wake_wins_over_entry_truncation_and_leaves_the_event_queued() {
    let mut machine = TestMachine::new(100_000_000, BOOT_VIRTUAL, &[]);
    machine.cpu.interrupt_word().host_wake_handle().request();
    let TestMachine {
        cpu,
        scheduler,
        bus,
        cpu_id,
    } = &mut machine;
    let burst = scheduler
        .begin_burst(100, cpu.interrupt_word().clone())
        .unwrap();
    scheduler.handle(TEST_DEVICE).schedule_at(50, 1, 0).unwrap();
    assert_ne!(cpu.interrupt_word().load_relaxed() & EVENT_TRUNCATE, 0);
    let exit = {
        let mut context = TestContext {
            scheduler,
            bus,
            cpu_id: *cpu_id,
            fail_synchronization: false,
        };
        cpu.run_until(&mut context, 100)
    };

    assert_eq!(exit, Ok(CpuExit::HostWake));
    drop(burst);
    assert_eq!(scheduler.now(), 0);
    assert_eq!(scheduler.front_time(), Some(50));
    assert_eq!(cpu.next_pclk_tick(), 1);
}

#[test]
fn guest_interrupt_line_is_ignored_and_never_cleared_in_m2() {
    let mut machine = TestMachine::new(100_000_000, BOOT_VIRTUAL, &[]);
    machine.install_program(&[0]);
    let sink = WordLineSink::new(machine.cpu.interrupt_word().clone(), 4).unwrap();
    sink.set(true);

    assert_eq!(machine.run_until(10), Ok(CpuExit::Deadline));
    assert_eq!(
        machine.cpu.interrupt_word().load_relaxed() & GUEST_INTERRUPT_MASK,
        1 << 4
    );
    assert_eq!(machine.cpu.next_pclk_tick(), 2);
}

#[test]
fn misaligned_instruction_fetch_takes_adel_without_touching_bus() {
    let bad_pc = BOOT_VIRTUAL + 2;
    let mut machine = TestMachine::new(100_000_000, bad_pc, &[]);

    assert_eq!(machine.run_until(10), Ok(CpuExit::Deadline));
    assert_eq!(machine.scheduler.now(), 10);
    assert_exception(&machine, ExceptionCode::AddressErrorLoad, bad_pc);
    assert_eq!(machine.cpu.next_pclk_tick(), 2);
    assert!(machine.bus.transactions.is_empty());
}

#[test]
fn both_physical_fetch_faults_become_instruction_bus_error() {
    for fault in [BusFault::Unmapped, BusFault::Fault] {
        let mut machine = TestMachine::new(100_000_000, BOOT_VIRTUAL, &[]);
        if fault == BusFault::Fault {
            machine.bus.read_faults.insert(BOOT_PHYSICAL, fault);
        }

        assert_eq!(machine.run_until(10), Ok(CpuExit::Deadline));
        assert_exception(&machine, ExceptionCode::InstructionBusError, 0);
        assert_eq!(machine.cpu.next_pclk_tick(), 2);
        assert_eq!(machine.bus.transactions.len(), 1);
    }
}

#[test]
fn lw_uses_one_timestamp_and_sign_extends_the_loaded_word() {
    let base = reg(1);
    let destination = reg(2);
    let mut machine = TestMachine::new(
        100_000_000,
        BOOT_VIRTUAL,
        &[(base, DATA_VIRTUAL), (destination, 0x55)],
    );
    machine.install_program(&[encode_i(0x23, 1, 2, 0)]);
    machine.bus.words.insert(DATA_PHYSICAL, 0x8000_0001);

    assert_eq!(machine.run_until(10), Ok(CpuExit::Deadline));
    assert_eq!(machine.cpu.read_gpr(destination), 0xffff_ffff_8000_0001);
    assert_eq!(machine.cpu.pc_state().current(), BOOT_VIRTUAL + 4);
    assert_eq!(machine.bus.transactions.len(), 2);
    assert!(
        machine
            .bus
            .transactions
            .iter()
            .all(|entry| entry.time == 10)
    );
    assert_eq!(
        machine.bus.transactions[1].address,
        PhysAddr::new(DATA_PHYSICAL)
    );
}

#[test]
fn sw_stores_the_low_word_before_committing_sequential_pc() {
    let base = reg(1);
    let source = reg(2);
    let mut machine = TestMachine::new(
        100_000_000,
        BOOT_VIRTUAL,
        &[(base, DATA_VIRTUAL), (source, 0x1234_5678_9abc_def0)],
    );
    machine.install_program(&[encode_i(0x2b, 1, 2, 0)]);

    assert_eq!(machine.run_until(10), Ok(CpuExit::Deadline));
    assert_eq!(machine.bus.words.get(&DATA_PHYSICAL), Some(&0x9abc_def0));
    assert_eq!(machine.cpu.pc_state().current(), BOOT_VIRTUAL + 4);
    assert_eq!(machine.cpu.next_pclk_tick(), 2);
    assert_eq!(machine.bus.transactions[1].kind, TransactionKind::Write32);
    assert_eq!(machine.bus.transactions[1].time, 10);
}

#[test]
fn word_alignment_faults_happen_before_data_bus_access() {
    for (opcode, expected) in [
        (0x23, ExceptionCode::AddressErrorLoad),
        (0x2b, ExceptionCode::AddressErrorStore),
    ] {
        let bad_address = DATA_VIRTUAL + 2;
        let mut machine = TestMachine::new(
            100_000_000,
            BOOT_VIRTUAL,
            &[(reg(1), bad_address), (reg(2), 0x1234)],
        );
        machine.install_program(&[encode_i(opcode, 1, 2, 0)]);

        assert_eq!(machine.run_until(10), Ok(CpuExit::Deadline));
        assert_exception(&machine, expected, bad_address);
        assert_eq!(machine.bus.transactions.len(), 1);
    }
}

#[test]
fn mips_iv_non_word_offset_is_an_execute_stop_even_if_effective_address_aligns() {
    let mut machine = TestMachine::new(100_000_000, BOOT_VIRTUAL, &[(reg(1), DATA_VIRTUAL + 3)]);
    let raw = encode_i(0x23, 1, 2, 1);
    machine.install_program(&[raw]);

    assert_eq!(
        machine.run_until(10),
        Err(CpuRunError::Execute(ExecuteError::UndefinedResult {
            instruction: Instruction::Lw {
                rt: reg(2),
                base: reg(1),
                immediate: 1,
            },
        }))
    );
    assert_eq!(machine.scheduler.now(), 10);
    assert_eq!(machine.cpu.next_pclk_tick(), 1);
    assert_eq!(machine.cpu.pc_state().current(), BOOT_VIRTUAL);
    assert_eq!(machine.bus.transactions.len(), 1);
}

#[test]
fn mapped_data_address_reports_tlb_gap_without_fake_translation() {
    let mut machine = TestMachine::new(100_000_000, BOOT_VIRTUAL, &[(reg(1), 0x1000)]);
    machine.install_program(&[encode_i(0x23, 1, 2, 0)]);

    assert_eq!(
        machine.run_until(10),
        Err(CpuRunError::Translation {
            access: TranslationAccess::LoadWord,
            error: TranslationError::TlbRequired {
                virtual_address: 0x1000,
            },
        })
    );
    assert_eq!(machine.cpu.next_pclk_tick(), 1);
    assert_eq!(machine.bus.transactions.len(), 1);
}

#[test]
fn data_bus_faults_take_dbe_and_do_not_apply_normal_cpu_commit() {
    let destination = reg(2);
    let mut load = TestMachine::new(
        100_000_000,
        BOOT_VIRTUAL,
        &[(reg(1), DATA_VIRTUAL), (destination, 0x55)],
    );
    load.install_program(&[encode_i(0x23, 1, 2, 0)]);
    assert_eq!(load.run_until(10), Ok(CpuExit::Deadline));
    assert_exception(&load, ExceptionCode::DataBusError, 0);
    assert_eq!(load.cpu.read_gpr(destination), 0x55);

    let mut store = TestMachine::new(
        100_000_000,
        BOOT_VIRTUAL,
        &[(reg(1), DATA_VIRTUAL), (reg(2), 0x1122_3344)],
    );
    store.install_program(&[encode_i(0x2b, 1, 2, 0)]);
    store
        .bus
        .write_faults
        .insert(DATA_PHYSICAL, BusFault::Fault);
    assert_eq!(store.run_until(10), Ok(CpuExit::Deadline));
    assert_exception(&store, ExceptionCode::DataBusError, 0);
    assert_eq!(store.bus.words.get(&DATA_PHYSICAL), Some(&0x1122_3344));
    assert_eq!(store.cpu.next_pclk_tick(), 2);
}

#[test]
fn faulting_mmio_with_scheduled_event_commits_exception_before_reschedule() {
    let mut machine = TestMachine::new(
        100_000_000,
        BOOT_VIRTUAL,
        &[(reg(1), DATA_VIRTUAL), (reg(2), 0xcafe_babe)],
    );
    machine.install_program(&[encode_i(0x2b, 1, 2, 0)]);
    machine.bus.schedule_on_write.insert(DATA_PHYSICAL, 10);
    machine
        .bus
        .write_faults
        .insert(DATA_PHYSICAL, BusFault::Fault);

    assert_eq!(machine.run_until(100), Ok(CpuExit::Reschedule));
    assert_eq!(machine.scheduler.now(), 10);
    assert_eq!(machine.scheduler.front_time(), Some(20));
    assert_eq!(machine.scheduler.pop_due(), Ok(None));
    assert_eq!(machine.bus.words.get(&DATA_PHYSICAL), Some(&0xcafe_babe));
    assert_exception(&machine, ExceptionCode::DataBusError, 0);
    assert_eq!(machine.cpu.next_pclk_tick(), 2);
    assert_eq!(machine.cpu.next_boundary(), Ok(20));
}

#[test]
fn context_failure_is_not_misclassified_as_guest_bus_error() {
    let mut machine = TestMachine::new(100_000_000, BOOT_VIRTUAL, &[]);
    machine.install_program(&[0]);
    let TestMachine {
        cpu,
        scheduler,
        bus,
        cpu_id,
    } = &mut machine;
    let _burst = scheduler
        .begin_burst(10, cpu.interrupt_word().clone())
        .unwrap();
    let mut context = TestContext {
        scheduler,
        bus,
        cpu_id: *cpu_id,
        fail_synchronization: true,
    };

    assert_eq!(
        cpu.run_until(&mut context, 10),
        Err(CpuRunError::Context(TestContextError::Injected))
    );
    assert_eq!(cpu.next_pclk_tick(), 1);
    assert_eq!(cpu.pc_state().current(), BOOT_VIRTUAL);
    assert!(context.bus.transactions.is_empty());
}

#[test]
fn timed_mmio_event_truncates_only_after_sw_and_phase_commit() {
    let mut machine = TestMachine::new(
        100_000_000,
        BOOT_VIRTUAL,
        &[(reg(1), DATA_VIRTUAL), (reg(2), 0xdead_beef)],
    );
    machine.install_program(&[encode_i(0x2b, 1, 2, 0)]);
    machine.bus.schedule_on_write.insert(DATA_PHYSICAL, 10);

    assert_eq!(machine.run_until(100), Ok(CpuExit::Reschedule));
    assert_eq!(machine.scheduler.now(), 10);
    assert_eq!(machine.scheduler.front_time(), Some(20));
    assert_eq!(machine.scheduler.pop_due(), Ok(None));
    assert_eq!(machine.bus.words.get(&DATA_PHYSICAL), Some(&0xdead_beef));
    assert_eq!(machine.cpu.pc_state().current(), BOOT_VIRTUAL + 4);
    assert_eq!(machine.cpu.next_pclk_tick(), 2);
    assert_eq!(machine.cpu.next_boundary(), Ok(20));
    assert_eq!(machine.bus.transactions[1].time, 10);
    assert_eq!(
        machine.cpu.interrupt_word().load_relaxed() & EVENT_TRUNCATE,
        0
    );
}

#[test]
fn zero_delay_mmio_event_remains_queued_after_complete_cpu_work_at_same_time() {
    let mut machine = TestMachine::new(
        100_000_000,
        BOOT_VIRTUAL,
        &[(reg(1), DATA_VIRTUAL), (reg(2), 0xa5a5_5a5a)],
    );
    machine.install_program(&[encode_i(0x2b, 1, 2, 0)]);
    machine.bus.schedule_on_write.insert(DATA_PHYSICAL, 0);

    assert_eq!(machine.run_until(100), Ok(CpuExit::Reschedule));
    assert_eq!(machine.scheduler.now(), 10);
    assert_eq!(machine.scheduler.front_time(), Some(10));
    assert_eq!(machine.cpu.pc_state().current(), BOOT_VIRTUAL + 4);
    assert_eq!(machine.cpu.next_pclk_tick(), 2);
    assert_eq!(
        machine.cpu.interrupt_word().load_relaxed() & EVENT_TRUNCATE,
        0
    );
    let event = machine.scheduler.pop_due().unwrap().unwrap();
    assert_eq!(event.vtime, 10);
}

#[test]
fn fetch_side_truncation_waits_until_the_fetched_instruction_completes() {
    let mut machine = TestMachine::new(100_000_000, BOOT_VIRTUAL, &[]);
    machine.install_program(&[encode_i(0x09, 0, 1, 42)]);
    machine.bus.schedule_on_read.insert(BOOT_PHYSICAL, 10);

    assert_eq!(machine.run_until(100), Ok(CpuExit::Reschedule));
    assert_eq!(machine.scheduler.now(), 10);
    assert_eq!(machine.scheduler.front_time(), Some(20));
    assert_eq!(machine.cpu.read_gpr(reg(1)), 42);
    assert_eq!(machine.cpu.pc_state().current(), BOOT_VIRTUAL + 4);
    assert_eq!(machine.cpu.next_pclk_tick(), 2);
}
