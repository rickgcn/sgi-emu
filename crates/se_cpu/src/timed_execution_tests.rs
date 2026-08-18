use std::cell::Cell;
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::rc::Rc;

use se_core::address::PhysAddr;
use se_core::bus::{Bus, BusFault, BusInitiator, CpuId, DirectAccess, DirectSpan};
use se_core::device::DeviceId;
use se_core::event::{EventQueueError, SchedulerHandle, SchedulerShared};
use se_core::interrupt::{
    EVENT_TRUNCATE, GUEST_INTERRUPT_MASK, HostWakeHandle, InterruptSink, WordLineSink,
};
use se_core::machine::CpuExit;
use se_core::time::VTime;

use crate::cp0::{Cp0, OperatingMode, SyntheticCp0State};
use crate::cpu::Cpu;
use crate::decode::Instruction;
use crate::exception::ExceptionCode;
use crate::execute::ExecuteError;
use crate::gpr::{GprFile, Reg};
use crate::pc::PcState;
use crate::run::{CpuRunContext, CpuRunError, TimedBusResult};
use crate::timing::{ProcessorClock, TimingError};

const CPU_ID: CpuId = CpuId::from_raw(7);
const TEST_DEVICE: DeviceId = DeviceId::from_raw(3);
const BOOT_VIRTUAL: u64 = 0xffff_ffff_bfc0_0000;
const BOOT_PHYSICAL: u64 = 0x1fc0_0000;
const DATA_VIRTUAL: u64 = 0xffff_ffff_a000_1000;
const DATA_PHYSICAL: u64 = 0x1000;
const MAPPED_VIRTUAL: u64 = 0x0040_0000;
const MAPPED_PHYSICAL: u64 = 0x4000;
const TLB_REFILL_PHYSICAL: u64 = 0;
const PTE_PAIR_PHYSICAL: u64 = 0x2000;
const CONTEXT_PTE_BASE: u64 = 0xffff_ffff_a000_0000;
const EXCEPTION_PHYSICAL: u64 = 0x180;
const IRQ_CLEAR_VIRTUAL: u64 = 0xffff_ffff_a000_2000;
const IRQ_CLEAR_PHYSICAL: u64 = 0x2000;
const ERET: u32 = 0x4200_0018;
const TLBWI: u32 = 0x4200_0002;

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

struct TestIrqDevice {
    pending: Cell<bool>,
    clear_writes: Cell<u32>,
    line: WordLineSink,
}

impl TestIrqDevice {
    fn new(line: WordLineSink) -> Self {
        Self {
            pending: Cell::new(false),
            clear_writes: Cell::new(0),
            line,
        }
    }

    fn set_pending(&self, pending: bool) {
        self.pending.set(pending);
        self.line.set(pending);
    }

    fn clear_from_mmio(&self, _value: u32) {
        self.clear_writes
            .set(self.clear_writes.get().wrapping_add(1));
        self.set_pending(false);
    }
}

struct TestBus {
    scheduler: SchedulerShared,
    schedule_handle: SchedulerHandle,
    words: BTreeMap<u64, u32>,
    read_faults: BTreeMap<u64, BusFault>,
    write_faults: BTreeMap<u64, BusFault>,
    schedule_on_read: BTreeMap<u64, VTime>,
    schedule_on_write: BTreeMap<u64, VTime>,
    irq_clear_devices: BTreeMap<u64, Rc<TestIrqDevice>>,
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
            irq_clear_devices: BTreeMap::new(),
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
        if let Some(device) = self.irq_clear_devices.get(&address.get()) {
            device.clear_from_mmio(value);
        }
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
    host_wake_on_synchronize: Option<(VTime, HostWakeHandle)>,
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
        self.scheduler.advance_to(time)?;

        if self
            .host_wake_on_synchronize
            .as_ref()
            .is_some_and(|(trigger, _)| time >= *trigger)
        {
            let (_, handle) = self
                .host_wake_on_synchronize
                .take()
                .expect("checked host-wake injection must exist");
            handle.request();
        }
        Ok(())
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
        Self::new_with_cp0(
            frequency_hz,
            entry_pc,
            initial_gprs,
            Cp0::synthetic_test_state(false),
        )
    }

    fn new_with_cp0(
        frequency_hz: u64,
        entry_pc: u64,
        initial_gprs: &[(Reg, u64)],
        cp0: Cp0,
    ) -> Self {
        let scheduler = SchedulerShared::new();
        let mut gpr = GprFile::new();
        for &(register, value) in initial_gprs {
            gpr.write(register, value);
        }
        let cpu = Cpu::from_parts(
            gpr,
            PcState::new(entry_pc),
            cp0,
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
        self.run_until_with_host_wake(deadline, None)
    }

    fn run_until_with_host_wake(
        &mut self,
        deadline: VTime,
        host_wake_at: Option<VTime>,
    ) -> Result<CpuExit, CpuRunError<TestContextError>> {
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
            host_wake_on_synchronize: host_wake_at
                .map(|time| (time, cpu.interrupt_word().host_wake_handle())),
        };
        cpu.run_until(&mut context, deadline)
    }

    fn install_program(&mut self, instructions: &[u32]) {
        self.install_at(BOOT_PHYSICAL, instructions);
    }

    fn install_handler(&mut self, instructions: &[u32]) {
        self.install_at(EXCEPTION_PHYSICAL, instructions);
    }

    fn install_at(&mut self, physical_base: u64, instructions: &[u32]) {
        for (index, instruction) in instructions.iter().copied().enumerate() {
            let offset = u64::try_from(index).unwrap() * 4;
            self.bus.words.insert(physical_base + offset, instruction);
        }
    }

    fn attach_irq_device(&mut self, line: u8) -> Rc<TestIrqDevice> {
        let sink = WordLineSink::new(self.cpu.interrupt_word().clone(), line).unwrap();
        let device = Rc::new(TestIrqDevice::new(sink));
        self.bus
            .irq_clear_devices
            .insert(IRQ_CLEAR_PHYSICAL, Rc::clone(&device));
        device
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

fn encode_cp0_move(rs: u8, rt: u8, register: u8) -> u32 {
    (0x10_u32 << 26) | (u32::from(rs) << 21) | (u32::from(rt) << 16) | (u32::from(register) << 11)
}

fn context_refill_cp0() -> Cp0 {
    Cp0::synthetic_test_state_with(
        SyntheticCp0State::new(false)
            .with_entry_hi_asid(0x42)
            .with_context_pte_base(CONTEXT_PTE_BASE)
            .with_xcontext_pte_base(0x1234_5600_0000_0000),
    )
}

fn entry_lo_word(physical_page: u64, valid: bool, dirty: bool, global: bool) -> u32 {
    assert!(physical_page.is_multiple_of(0x1000));
    let pfn = u32::try_from(physical_page >> 12).expect("test PFN must fit in a PTE word");
    (pfn << 6) | (u32::from(dirty) << 2) | (u32::from(valid) << 1) | u32::from(global)
}

fn install_context_refill_handler(machine: &mut TestMachine, index: u8) {
    machine.install_at(
        TLB_REFILL_PHYSICAL,
        &[
            encode_cp0_move(0, 26, 4),
            encode_i(0x23, 26, 27, 0),
            encode_cp0_move(4, 27, 2),
            encode_i(0x23, 26, 27, 4),
            encode_cp0_move(4, 27, 3),
            encode_i(0x09, 0, 27, u16::from(index)),
            encode_cp0_move(4, 27, 0),
            TLBWI,
            ERET,
        ],
    );
    machine.bus.words.insert(
        PTE_PAIR_PHYSICAL,
        entry_lo_word(MAPPED_PHYSICAL, true, true, false),
    );
    machine.bus.words.insert(
        PTE_PAIR_PHYSICAL + 4,
        entry_lo_word(MAPPED_PHYSICAL + 0x1000, true, true, false),
    );
}

fn interrupt_enabled_cp0(mask: u8) -> Cp0 {
    Cp0::synthetic_test_state_with(
        SyntheticCp0State::new(false)
            .with_interrupts(true, mask)
            .with_operating_mode(OperatingMode::Kernel, false),
    )
}

fn install_clearing_irq_handler(machine: &mut TestMachine, clear_base: u8) {
    machine.install_handler(&[encode_i(0x2b, clear_base, 0, 0), ERET]);
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
        host_wake_on_synchronize: None,
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
            host_wake_on_synchronize: None,
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
fn disabled_guest_interrupt_remains_pending_without_acceptance() {
    let mut machine = TestMachine::new(100_000_000, BOOT_VIRTUAL, &[]);
    machine.install_program(&[0]);
    let sink = WordLineSink::new(machine.cpu.interrupt_word().clone(), 4).unwrap();
    sink.set(true);

    assert_eq!(machine.run_until(10), Ok(CpuExit::Deadline));
    assert_eq!(
        machine.cpu.interrupt_word().load_relaxed() & GUEST_INTERRUPT_MASK,
        1 << 4
    );
    assert_eq!(machine.cpu.cause_pending_ip(), 1 << 6);
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
fn mapped_data_address_takes_refill_before_target_bus_access() {
    let mut machine = TestMachine::new(100_000_000, BOOT_VIRTUAL, &[(reg(1), 0x1000)]);
    machine.install_program(&[encode_i(0x23, 1, 2, 0)]);

    assert_eq!(machine.run_until(10), Ok(CpuExit::Deadline));
    assert_eq!(machine.cpu.cp0().exception_code(), ExceptionCode::TlbLoad);
    assert_eq!(machine.cpu.cp0().bad_vaddr(), 0x1000);
    assert_eq!(machine.cpu.pc_state().current(), 0xffff_ffff_8000_0000);
    assert_eq!(machine.cpu.next_pclk_tick(), 2);
    assert_eq!(machine.bus.transactions.len(), 1);
}

#[test]
fn guest_refill_handler_installs_mapping_and_retries_load() {
    let base = reg(1);
    let destination = reg(2);
    let mut machine = TestMachine::new_with_cp0(
        100_000_000,
        BOOT_VIRTUAL,
        &[(base, MAPPED_VIRTUAL), (destination, 0x55)],
        context_refill_cp0(),
    );
    machine.install_program(&[encode_i(0x23, 1, 2, 0)]);
    install_context_refill_handler(&mut machine, 9);
    machine.bus.words.insert(MAPPED_PHYSICAL, 0x8000_0001);

    assert_eq!(machine.run_until(110), Ok(CpuExit::Deadline));

    assert_eq!(machine.cpu.read_gpr(destination), 0xffff_ffff_8000_0001);
    assert_eq!(machine.cpu.pc_state().current(), BOOT_VIRTUAL + 4);
    assert_eq!(machine.cpu.cp0().epc(), BOOT_VIRTUAL);
    assert!(!machine.cpu.cp0().exl());
    assert!(!machine.cpu.cp0().tlb_shutdown());
    assert_eq!(machine.cpu.cp0().entry_hi().asid(), 0x42);
    assert_eq!(
        machine
            .bus
            .transactions
            .iter()
            .filter(|transaction| {
                transaction.kind == TransactionKind::Read32
                    && transaction.address == PhysAddr::new(MAPPED_PHYSICAL)
            })
            .count(),
        1
    );
    assert_eq!(
        machine
            .bus
            .transactions
            .iter()
            .filter(|transaction| {
                transaction.kind == TransactionKind::Read32
                    && matches!(transaction.address.get(), PTE_PAIR_PHYSICAL | 0x2004)
            })
            .count(),
        2
    );
}

#[test]
fn guest_refill_handler_retries_mapped_instruction_fetch() {
    let mut machine =
        TestMachine::new_with_cp0(100_000_000, MAPPED_VIRTUAL, &[], context_refill_cp0());
    install_context_refill_handler(&mut machine, 10);
    machine
        .bus
        .words
        .insert(MAPPED_PHYSICAL, encode_i(0x09, 0, 5, 7));

    assert_eq!(machine.run_until(110), Ok(CpuExit::Deadline));

    assert_eq!(machine.cpu.read_gpr(reg(5)), 7);
    assert_eq!(machine.cpu.pc_state().current(), MAPPED_VIRTUAL + 4);
    assert_eq!(machine.cpu.cp0().epc(), MAPPED_VIRTUAL);
    assert!(!machine.cpu.cp0().exl());
    let target_reads: Vec<_> = machine
        .bus
        .transactions
        .iter()
        .filter(|transaction| {
            transaction.kind == TransactionKind::Read32
                && transaction.address == PhysAddr::new(MAPPED_PHYSICAL)
        })
        .collect();
    assert_eq!(target_reads.len(), 1);
    assert_eq!(target_reads[0].time, 110);
}

#[test]
fn delay_slot_store_refill_restarts_branch_and_writes_once() {
    let base = reg(1);
    let source = reg(2);
    let mut machine = TestMachine::new_with_cp0(
        100_000_000,
        BOOT_VIRTUAL,
        &[(base, MAPPED_VIRTUAL), (source, 0x1234_5678_9abc_def0)],
        context_refill_cp0(),
    );
    machine.install_program(&[
        encode_i(0x04, 0, 0, 3),
        encode_i(0x2b, 1, 2, 0),
        encode_i(0x09, 7, 7, 1),
        encode_i(0x09, 7, 7, 1),
        encode_i(0x09, 6, 6, 1),
    ]);
    install_context_refill_handler(&mut machine, 11);

    assert_eq!(machine.run_until(140), Ok(CpuExit::Deadline));

    assert_eq!(machine.bus.words.get(&MAPPED_PHYSICAL), Some(&0x9abc_def0));
    assert_eq!(machine.cpu.read_gpr(reg(6)), 1);
    assert_eq!(machine.cpu.read_gpr(reg(7)), 0);
    assert_eq!(machine.cpu.pc_state().current(), BOOT_VIRTUAL + 20);
    assert_eq!(machine.cpu.cp0().epc(), BOOT_VIRTUAL);
    assert!(machine.cpu.cp0().branch_delay());
    assert!(!machine.cpu.cp0().exl());
    let target_transactions: Vec<_> = machine
        .bus
        .transactions
        .iter()
        .filter(|transaction| transaction.address == PhysAddr::new(MAPPED_PHYSICAL))
        .collect();
    assert_eq!(target_transactions.len(), 1);
    assert_eq!(target_transactions[0].kind, TransactionKind::Write32);
    assert_eq!(target_transactions[0].time, 130);
    assert_eq!(
        machine
            .bus
            .transactions
            .iter()
            .filter(|transaction| {
                transaction.kind == TransactionKind::Read32
                    && transaction.address == PhysAddr::new(BOOT_PHYSICAL)
            })
            .count(),
        2
    );
    assert_eq!(
        machine
            .bus
            .transactions
            .iter()
            .filter(|transaction| {
                transaction.kind == TransactionKind::Read32
                    && transaction.address == PhysAddr::new(BOOT_PHYSICAL + 4)
            })
            .count(),
        2
    );
}

#[test]
fn illegal_fetch_addresses_fault_before_translation_and_bus_access() {
    let cases = [
        (0x0000_0000_8000_0000, Cp0::synthetic_test_state(false)),
        (
            BOOT_VIRTUAL,
            Cp0::synthetic_test_state_with(
                SyntheticCp0State::new(false).with_operating_mode(OperatingMode::User, false),
            ),
        ),
    ];

    for (bad_pc, cp0) in cases {
        let mut machine = TestMachine::new_with_cp0(100_000_000, bad_pc, &[], cp0);

        assert_eq!(machine.run_until(10), Ok(CpuExit::Deadline));
        assert_exception(&machine, ExceptionCode::AddressErrorLoad, bad_pc);
        assert!(machine.bus.transactions.is_empty());
    }
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
        host_wake_on_synchronize: None,
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

#[test]
fn equal_boundary_control_exit_leaves_cpu_work_pending_for_reentry() {
    let mut machine = TestMachine::new(100_000_000, BOOT_VIRTUAL, &[]);
    machine.install_program(&[encode_i(0x09, 0, 1, 7)]);
    machine.scheduler.advance_to(10).unwrap();
    machine.cpu.interrupt_word().host_wake_handle().request();

    assert_eq!(machine.run_until(10), Ok(CpuExit::HostWake));
    assert_eq!(machine.scheduler.now(), 10);
    assert_eq!(machine.cpu.next_pclk_tick(), 1);
    assert_eq!(machine.cpu.read_gpr(reg(1)), 0);
    assert!(machine.bus.transactions.is_empty());

    assert_eq!(machine.run_until(10), Ok(CpuExit::Deadline));
    assert_eq!(machine.cpu.next_pclk_tick(), 2);
    assert_eq!(machine.cpu.read_gpr(reg(1)), 7);
    assert_eq!(machine.cpu.pc_state().current(), BOOT_VIRTUAL + 4);
}

#[test]
fn host_wake_during_off_grid_deadline_sync_is_deferred_to_reentry() {
    let mut machine = TestMachine::new(100_000_000, BOOT_VIRTUAL, &[]);

    assert_eq!(
        machine.run_until_with_host_wake(5, Some(5)),
        Ok(CpuExit::Deadline)
    );
    assert_eq!(machine.scheduler.now(), 5);
    assert_eq!(machine.cpu.next_pclk_tick(), 1);

    assert_eq!(machine.run_until(5), Ok(CpuExit::HostWake));
    assert_eq!(machine.scheduler.now(), 5);
    assert_eq!(machine.cpu.next_pclk_tick(), 1);
}

#[test]
fn host_wake_during_boundary_sync_follows_the_authorized_instruction() {
    let mut machine = TestMachine::new(100_000_000, BOOT_VIRTUAL, &[]);
    machine.install_program(&[encode_i(0x09, 0, 1, 7)]);

    assert_eq!(
        machine.run_until_with_host_wake(100, Some(10)),
        Ok(CpuExit::HostWake)
    );
    assert_eq!(machine.scheduler.now(), 10);
    assert_eq!(machine.cpu.next_pclk_tick(), 2);
    assert_eq!(machine.cpu.read_gpr(reg(1)), 7);
    assert_eq!(machine.cpu.pc_state().current(), BOOT_VIRTUAL + 4);
    assert_eq!(machine.bus.transactions.len(), 1);
}

#[test]
fn pending_view_stays_live_when_status_blocks_interrupt_acceptance() {
    let states = [
        SyntheticCp0State::new(false).with_interrupts(false, 1 << 2),
        SyntheticCp0State::new(false).with_interrupts(true, 0),
        SyntheticCp0State::new(false)
            .with_interrupts(true, 1 << 2)
            .with_exception_levels(true, false),
        SyntheticCp0State::new(false)
            .with_interrupts(true, 1 << 2)
            .with_exception_levels(false, true),
    ];

    for state in states {
        let mut machine = TestMachine::new_with_cp0(
            100_000_000,
            BOOT_VIRTUAL,
            &[],
            Cp0::synthetic_test_state_with(state),
        );
        machine.install_program(&[0]);
        let sink = WordLineSink::new(machine.cpu.interrupt_word().clone(), 0).unwrap();
        sink.set(true);

        assert_eq!(machine.cpu.cause_pending_ip(), 1 << 2);
        assert_eq!(machine.run_until(10), Ok(CpuExit::Deadline));
        assert_eq!(machine.cpu.cause_pending_ip(), 1 << 2);
        assert_eq!(
            machine.cpu.cp0().exception_code(),
            ExceptionCode::ReservedInstruction
        );
        assert_eq!(machine.cpu.pc_state().current(), BOOT_VIRTUAL + 4);
        assert_eq!(machine.cpu.next_pclk_tick(), 2);
        assert_eq!(machine.bus.transactions.len(), 1);
    }
}

#[test]
fn multiple_external_lines_take_one_generic_interrupt_without_clearing_levels() {
    let mut machine =
        TestMachine::new_with_cp0(100_000_000, BOOT_VIRTUAL, &[], interrupt_enabled_cp0(0x7c));
    let sinks: Vec<_> = (0..5)
        .map(|line| WordLineSink::new(machine.cpu.interrupt_word().clone(), line).unwrap())
        .collect();
    for sink in &sinks {
        sink.set(true);
    }

    assert_eq!(machine.cpu.cause_pending_ip(), 0x7c);
    assert_eq!(machine.run_until(10), Ok(CpuExit::Deadline));

    assert_eq!(machine.cpu.cp0().exception_code(), ExceptionCode::Interrupt);
    assert_eq!(machine.cpu.cp0().epc(), BOOT_VIRTUAL);
    assert!(!machine.cpu.cp0().branch_delay());
    assert_eq!(machine.cpu.cause_pending_ip(), 0x7c);
    assert_eq!(
        machine.cpu.interrupt_word().load_relaxed() & GUEST_INTERRUPT_MASK,
        0x1f
    );
    assert_eq!(machine.cpu.next_pclk_tick(), 2);
    assert!(machine.bus.transactions.is_empty());
}

#[test]
fn off_grid_irq_assertion_waits_for_the_next_architectural_boundary() {
    let mut machine = TestMachine::new_with_cp0(
        100_000_000,
        BOOT_VIRTUAL,
        &[],
        interrupt_enabled_cp0(1 << 2),
    );
    machine.install_program(&[0]);
    machine.scheduler.advance_to(5).unwrap();
    let sink = WordLineSink::new(machine.cpu.interrupt_word().clone(), 0).unwrap();
    sink.set(true);

    assert_eq!(machine.run_until(9), Ok(CpuExit::Deadline));
    assert_eq!(machine.scheduler.now(), 9);
    assert_eq!(machine.cpu.next_pclk_tick(), 1);
    assert_eq!(machine.cpu.pc_state().current(), BOOT_VIRTUAL);
    assert!(machine.bus.transactions.is_empty());

    assert_eq!(machine.run_until(10), Ok(CpuExit::Deadline));
    assert_eq!(machine.cpu.cp0().exception_code(), ExceptionCode::Interrupt);
    assert_eq!(machine.cpu.next_pclk_tick(), 2);
    assert!(machine.bus.transactions.is_empty());
}

#[test]
fn host_wake_during_boundary_sync_follows_the_authorized_irq() {
    let mut machine = TestMachine::new_with_cp0(
        100_000_000,
        BOOT_VIRTUAL,
        &[],
        interrupt_enabled_cp0(1 << 2),
    );
    let sink = WordLineSink::new(machine.cpu.interrupt_word().clone(), 0).unwrap();
    sink.set(true);

    assert_eq!(
        machine.run_until_with_host_wake(100, Some(10)),
        Ok(CpuExit::HostWake)
    );
    assert_eq!(machine.scheduler.now(), 10);
    assert_eq!(machine.cpu.next_pclk_tick(), 2);
    assert_eq!(machine.cpu.cp0().exception_code(), ExceptionCode::Interrupt);
    assert_eq!(machine.cpu.cp0().epc(), BOOT_VIRTUAL);
    assert_eq!(machine.cpu.pc_state().current(), 0xffff_ffff_8000_0180);
    assert_eq!(machine.cpu.cause_pending_ip(), 1 << 2);
    assert!(machine.bus.transactions.is_empty());
}

#[test]
fn entry_event_truncation_beats_an_eligible_irq() {
    let mut machine = TestMachine::new_with_cp0(
        100_000_000,
        BOOT_VIRTUAL,
        &[],
        interrupt_enabled_cp0(1 << 2),
    );
    let sink = WordLineSink::new(machine.cpu.interrupt_word().clone(), 0).unwrap();
    sink.set(true);
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
            host_wake_on_synchronize: None,
        };
        cpu.run_until(&mut context, 100)
    };

    assert_eq!(exit, Ok(CpuExit::Reschedule));
    drop(burst);
    assert_eq!(scheduler.now(), 0);
    assert_eq!(scheduler.front_time(), Some(50));
    assert_eq!(cpu.next_pclk_tick(), 1);
    assert_eq!(cpu.pc_state().current(), BOOT_VIRTUAL);
    assert_eq!(
        cpu.cp0().exception_code(),
        ExceptionCode::ReservedInstruction
    );
    assert_eq!(cpu.cause_pending_ip(), 1 << 2);
    assert!(bus.transactions.is_empty());
}

#[test]
fn equal_time_event_assertion_affects_only_a_later_cpu_boundary() {
    let mut asserted_before = TestMachine::new_with_cp0(
        100_000_000,
        BOOT_VIRTUAL,
        &[],
        interrupt_enabled_cp0(1 << 2),
    );
    let before_device = asserted_before.attach_irq_device(0);
    asserted_before
        .scheduler
        .handle(TEST_DEVICE)
        .schedule_at(5, 1, 0)
        .unwrap();

    assert_eq!(asserted_before.run_until(5), Ok(CpuExit::Deadline));
    assert!(asserted_before.scheduler.pop_due().unwrap().is_some());
    before_device.set_pending(true);
    assert_eq!(asserted_before.run_until(10), Ok(CpuExit::Deadline));
    assert_eq!(
        asserted_before.cpu.cp0().exception_code(),
        ExceptionCode::Interrupt
    );
    assert!(asserted_before.bus.transactions.is_empty());

    let mut asserted_at = TestMachine::new_with_cp0(
        100_000_000,
        BOOT_VIRTUAL,
        &[],
        interrupt_enabled_cp0(1 << 2),
    );
    asserted_at.install_program(&[encode_i(0x09, 0, 1, 7)]);
    let at_device = asserted_at.attach_irq_device(0);
    asserted_at
        .scheduler
        .handle(TEST_DEVICE)
        .schedule_at(10, 1, 0)
        .unwrap();

    assert_eq!(asserted_at.run_until(10), Ok(CpuExit::Deadline));
    assert_eq!(asserted_at.cpu.read_gpr(reg(1)), 7);
    assert_eq!(asserted_at.cpu.pc_state().current(), BOOT_VIRTUAL + 4);
    assert_eq!(asserted_at.cpu.next_pclk_tick(), 2);
    assert!(asserted_at.scheduler.pop_due().unwrap().is_some());
    at_device.set_pending(true);

    assert_eq!(asserted_at.run_until(20), Ok(CpuExit::Deadline));
    assert_eq!(
        asserted_at.cpu.cp0().exception_code(),
        ExceptionCode::Interrupt
    );
    assert_eq!(asserted_at.cpu.cp0().epc(), BOOT_VIRTUAL + 4);
    assert_eq!(asserted_at.cpu.next_pclk_tick(), 3);
    assert_eq!(asserted_at.bus.transactions.len(), 1);
}

#[test]
fn asserted_source_reenters_interrupt_after_eret() {
    let mut machine = TestMachine::new_with_cp0(
        100_000_000,
        BOOT_VIRTUAL,
        &[],
        interrupt_enabled_cp0(1 << 2),
    );
    machine.install_handler(&[ERET]);
    machine.install_program(&[encode_i(0x09, 0, 1, 1)]);
    let sink = WordLineSink::new(machine.cpu.interrupt_word().clone(), 0).unwrap();
    sink.set(true);

    assert_eq!(machine.run_until(10), Ok(CpuExit::Deadline));
    assert_eq!(machine.cpu.cp0().epc(), BOOT_VIRTUAL);
    assert!(machine.cpu.cp0().exl());
    assert_eq!(machine.cpu.next_pclk_tick(), 2);

    assert_eq!(machine.run_until(20), Ok(CpuExit::Deadline));
    assert!(!machine.cpu.cp0().exl());
    assert_eq!(machine.cpu.pc_state().current(), BOOT_VIRTUAL);
    assert_eq!(machine.cpu.next_pclk_tick(), 3);

    assert_eq!(machine.run_until(30), Ok(CpuExit::Deadline));
    assert!(machine.cpu.cp0().exl());
    assert_eq!(machine.cpu.cp0().exception_code(), ExceptionCode::Interrupt);
    assert_eq!(machine.cpu.cp0().epc(), BOOT_VIRTUAL);
    assert_eq!(machine.cpu.read_gpr(reg(1)), 0);
    assert_eq!(machine.cpu.cause_pending_ip(), 1 << 2);
    assert_eq!(machine.cpu.next_pclk_tick(), 4);
    assert_eq!(machine.bus.transactions.len(), 1);
}

#[test]
fn ordinary_irq_handler_clears_returns_and_resumes_exactly_once() {
    let clear_base = reg(20);
    let mut machine = TestMachine::new_with_cp0(
        100_000_000,
        BOOT_VIRTUAL,
        &[(clear_base, IRQ_CLEAR_VIRTUAL)],
        interrupt_enabled_cp0(1 << 2),
    );
    machine.install_program(&[encode_i(0x09, 0, 1, 1), encode_i(0x09, 2, 2, 1), 0]);
    install_clearing_irq_handler(&mut machine, 20);
    let device = machine.attach_irq_device(0);
    machine
        .scheduler
        .handle(TEST_DEVICE)
        .schedule_at(15, 1, 0)
        .unwrap();

    assert_eq!(machine.run_until(15), Ok(CpuExit::Deadline));
    assert_eq!(machine.cpu.read_gpr(reg(1)), 1);
    assert_eq!(machine.cpu.read_gpr(reg(2)), 0);
    assert_eq!(machine.cpu.next_pclk_tick(), 2);
    assert!(machine.scheduler.pop_due().unwrap().is_some());
    device.set_pending(true);
    assert!(device.pending.get());
    assert_eq!(machine.cpu.cause_pending_ip(), 1 << 2);

    assert_eq!(machine.run_until(20), Ok(CpuExit::Deadline));
    assert_eq!(machine.cpu.cp0().exception_code(), ExceptionCode::Interrupt);
    assert_eq!(machine.cpu.cp0().epc(), BOOT_VIRTUAL + 4);
    assert!(!machine.cpu.cp0().branch_delay());
    assert_eq!(machine.cpu.pc_state().current(), 0xffff_ffff_8000_0180);
    assert_eq!(machine.cpu.next_pclk_tick(), 3);
    assert!(device.pending.get());
    assert_eq!(device.clear_writes.get(), 0);
    assert_eq!(machine.bus.transactions.len(), 1);

    assert_eq!(machine.run_until(30), Ok(CpuExit::Deadline));
    assert!(!device.pending.get());
    assert_eq!(device.clear_writes.get(), 1);
    assert_eq!(machine.cpu.cause_pending_ip(), 0);
    assert_eq!(machine.cpu.pc_state().current(), 0xffff_ffff_8000_0184);
    assert_eq!(machine.cpu.next_pclk_tick(), 4);

    assert_eq!(machine.run_until(40), Ok(CpuExit::Deadline));
    assert!(!machine.cpu.cp0().exl());
    assert_eq!(machine.cpu.pc_state().current(), BOOT_VIRTUAL + 4);
    assert_eq!(machine.cpu.pc_state().next(), BOOT_VIRTUAL + 8);
    assert_eq!(machine.cpu.next_pclk_tick(), 5);

    assert_eq!(machine.run_until(50), Ok(CpuExit::Deadline));
    assert_eq!(machine.cpu.read_gpr(reg(2)), 1);
    assert_eq!(machine.cpu.pc_state().current(), BOOT_VIRTUAL + 8);
    assert_eq!(machine.cpu.next_pclk_tick(), 6);
    assert_eq!(
        machine
            .bus
            .transactions
            .iter()
            .filter(|transaction| {
                transaction.kind == TransactionKind::Read32
                    && transaction.address == PhysAddr::new(BOOT_PHYSICAL + 4)
            })
            .count(),
        1
    );
}

#[test]
fn delay_slot_irq_handler_restarts_branch_and_executes_slot_once() {
    let clear_base = reg(20);
    let mut machine = TestMachine::new_with_cp0(
        100_000_000,
        BOOT_VIRTUAL,
        &[(clear_base, IRQ_CLEAR_VIRTUAL)],
        interrupt_enabled_cp0(1 << 2),
    );
    machine.install_program(&[
        encode_i(0x04, 0, 0, 3),
        encode_i(0x09, 5, 5, 1),
        encode_i(0x09, 7, 7, 1),
        encode_i(0x09, 7, 7, 1),
        encode_i(0x09, 6, 6, 1),
    ]);
    install_clearing_irq_handler(&mut machine, 20);
    let device = machine.attach_irq_device(0);
    machine
        .scheduler
        .handle(TEST_DEVICE)
        .schedule_at(15, 1, 0)
        .unwrap();

    assert_eq!(machine.run_until(15), Ok(CpuExit::Deadline));
    assert_eq!(machine.cpu.pc_state().current(), BOOT_VIRTUAL + 4);
    assert_eq!(machine.cpu.pc_state().next(), BOOT_VIRTUAL + 16);
    assert_eq!(machine.cpu.pc_state().delay_slot_of(), Some(BOOT_VIRTUAL));
    assert_eq!(machine.cpu.read_gpr(reg(5)), 0);
    assert!(machine.scheduler.pop_due().unwrap().is_some());
    device.set_pending(true);

    assert_eq!(machine.run_until(20), Ok(CpuExit::Deadline));
    assert_eq!(machine.cpu.cp0().exception_code(), ExceptionCode::Interrupt);
    assert_eq!(machine.cpu.cp0().epc(), BOOT_VIRTUAL);
    assert!(machine.cpu.cp0().branch_delay());
    assert_eq!(machine.cpu.read_gpr(reg(5)), 0);
    assert_eq!(machine.cpu.next_pclk_tick(), 3);
    assert_eq!(machine.bus.transactions.len(), 1);
    assert!(device.pending.get());

    assert_eq!(machine.run_until(30), Ok(CpuExit::Deadline));
    assert!(!device.pending.get());
    assert_eq!(device.clear_writes.get(), 1);
    assert_eq!(machine.cpu.next_pclk_tick(), 4);

    assert_eq!(machine.run_until(40), Ok(CpuExit::Deadline));
    assert_eq!(machine.cpu.pc_state().current(), BOOT_VIRTUAL);
    assert_eq!(machine.cpu.pc_state().next(), BOOT_VIRTUAL + 4);
    assert_eq!(machine.cpu.pc_state().delay_slot_of(), None);
    assert_eq!(machine.cpu.next_pclk_tick(), 5);

    assert_eq!(machine.run_until(50), Ok(CpuExit::Deadline));
    assert_eq!(machine.cpu.pc_state().current(), BOOT_VIRTUAL + 4);
    assert_eq!(machine.cpu.pc_state().next(), BOOT_VIRTUAL + 16);
    assert_eq!(machine.cpu.pc_state().delay_slot_of(), Some(BOOT_VIRTUAL));

    assert_eq!(machine.run_until(60), Ok(CpuExit::Deadline));
    assert_eq!(machine.cpu.read_gpr(reg(5)), 1);
    assert_eq!(machine.cpu.pc_state().current(), BOOT_VIRTUAL + 16);
    assert_eq!(machine.cpu.pc_state().delay_slot_of(), None);

    assert_eq!(machine.run_until(70), Ok(CpuExit::Deadline));
    assert_eq!(machine.cpu.read_gpr(reg(5)), 1);
    assert_eq!(machine.cpu.read_gpr(reg(6)), 1);
    assert_eq!(machine.cpu.read_gpr(reg(7)), 0);
    assert_eq!(machine.cpu.next_pclk_tick(), 8);
    assert_eq!(
        machine
            .bus
            .transactions
            .iter()
            .filter(|transaction| {
                transaction.kind == TransactionKind::Read32
                    && transaction.address == PhysAddr::new(BOOT_PHYSICAL)
            })
            .count(),
        2
    );
    assert_eq!(
        machine
            .bus
            .transactions
            .iter()
            .filter(|transaction| {
                transaction.kind == TransactionKind::Read32
                    && transaction.address == PhysAddr::new(BOOT_PHYSICAL + 4)
            })
            .count(),
        1
    );
}

#[test]
fn eret_in_a_delay_slot_stops_before_cp0_usability_without_advancing_phase() {
    let cp0 = Cp0::synthetic_test_state_with(
        SyntheticCp0State::new(false).with_return_addresses(BOOT_VIRTUAL + 8, 0),
    );
    let mut machine = TestMachine::new_with_cp0(100_000_000, BOOT_VIRTUAL, &[], cp0);
    machine.install_program(&[encode_i(0x04, 0, 0, 1), ERET, 0]);

    assert_eq!(machine.run_until(10), Ok(CpuExit::Deadline));
    let before = machine.cpu.clone();

    assert_eq!(
        machine.run_until(20),
        Err(CpuRunError::Execute(
            ExecuteError::UnpredictableControlFlow {
                instruction_pc: BOOT_VIRTUAL + 4,
                branch_pc: BOOT_VIRTUAL,
            }
        ))
    );
    assert_eq!(machine.scheduler.now(), 20);
    assert_eq!(machine.cpu, before);
    assert_eq!(machine.cpu.next_pclk_tick(), 2);
    assert_eq!(machine.cpu.pc_state().delay_slot_of(), Some(BOOT_VIRTUAL));
}
