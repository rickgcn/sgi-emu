//! Private native ABI for MIPS IV host-code backends.

use std::panic::{AssertUnwindSafe, catch_unwind};

use se_device::cpu::mips4::cache::Mips4MemoryAccessType;
use se_device::cpu::mips4::cp1::decode::Mips4Cp1Decode;
use se_device::cpu::mips4::exception::Mips4Exception;
use se_device::cpu::mips4::execution::block::*;
use se_device::cpu::mips4::execution::bus::{Mips4ExecutionAccessKind, Mips4ExecutionTransferSize};

use super::fast_memory::{Mips4FastMemoryContext, Mips4NativeFastMemoryRuntime};
use super::region::Mips4RegionSideExit;

/// Stable runtime descriptor tag exposed through the native frame ABI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub(super) enum Mips4RuntimeOperationTag {
    /// Integer memory access.
    Memory = 1,
    /// Integer prefetch.
    Prefetch = 2,
    /// CP0, TLB, ERET, or WAIT operation.
    Cp0 = 3,
    /// CP1 operation.
    Cp1 = 4,
    /// Processor-specific CACHE operation.
    Cache = 5,
    /// CP2 or CP3 access.
    Coprocessor = 6,
    /// Preselected architectural exception.
    Raise = 7,
}

/// Stable normalized runtime operation record referenced by native blocks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub(super) struct Mips4RuntimeOperationDescriptor {
    tag: Mips4RuntimeOperationTag,
    flags: u32,
    operands: [u64; 6],
}

impl Mips4RuntimeOperationDescriptor {
    fn from_operation(operation: Mips4RuntimeOperation) -> Self {
        let mut descriptor = Self {
            tag: Mips4RuntimeOperationTag::Raise,
            flags: 0,
            operands: [0; 6],
        };
        match operation {
            Mips4RuntimeOperation::Memory { instruction, raw } => {
                descriptor.tag = Mips4RuntimeOperationTag::Memory;
                descriptor.operands = [
                    instruction as u64,
                    u64::from(raw.bits()),
                    u64::from(raw.rs()),
                    u64::from(raw.rt()),
                    raw.signed_immediate() as i64 as u64,
                    0,
                ];
            }
            Mips4RuntimeOperation::Prefetch { raw } => {
                descriptor.tag = Mips4RuntimeOperationTag::Prefetch;
                descriptor.operands = [
                    u64::from(raw.bits()),
                    u64::from(raw.rs()),
                    u64::from(raw.rt()),
                    raw.signed_immediate() as i64 as u64,
                    0,
                    0,
                ];
            }
            Mips4RuntimeOperation::Cp0 { raw, operation } => {
                descriptor.tag = Mips4RuntimeOperationTag::Cp0;
                descriptor.operands[0] = u64::from(raw.bits());
                descriptor.operands[1] = cp0_operation_code(operation);
            }
            Mips4RuntimeOperation::Cp1 { raw, decoded } => {
                descriptor.tag = Mips4RuntimeOperationTag::Cp1;
                descriptor.operands[0] = u64::from(raw.bits());
                descriptor.operands[1] = match decoded {
                    Mips4Cp1Decode::Instruction(_) => 1,
                    Mips4Cp1Decode::ReservedOrUnimplementedOperation => 2,
                };
            }
            Mips4RuntimeOperation::Cache {
                raw,
                base,
                offset,
                selector,
                operation,
            } => {
                descriptor.tag = Mips4RuntimeOperationTag::Cache;
                descriptor.operands = [
                    u64::from(raw.bits()),
                    u64::from(base),
                    offset as i64 as u64,
                    u64::from(selector),
                    u64::from(operation),
                    0,
                ];
            }
            Mips4RuntimeOperation::Coprocessor {
                coprocessor,
                requirements,
            } => {
                descriptor.tag = Mips4RuntimeOperationTag::Coprocessor;
                descriptor.operands[0] = u64::from(coprocessor.number());
                descriptor.operands[1] = architecture_level_code(requirements.architecture_level);
                descriptor.operands[2] = disabled_action_code(requirements.disabled_action);
            }
            Mips4RuntimeOperation::Raise(exception) => {
                descriptor.tag = Mips4RuntimeOperationTag::Raise;
                descriptor.operands[0] = u64::from(exception.cause_code());
                if let Mips4Exception::CoprocessorUnusable { coprocessor } = exception {
                    descriptor.operands[1] = u64::from(coprocessor.number());
                }
            }
        }
        descriptor
    }
}

const fn architecture_level_code(
    level: se_device::cpu::mips4::instruction::requirements::Mips4ArchitectureLevel,
) -> u64 {
    use se_device::cpu::mips4::instruction::requirements::Mips4ArchitectureLevel;

    match level {
        Mips4ArchitectureLevel::Mips1 => 1,
        Mips4ArchitectureLevel::Mips2 => 2,
        Mips4ArchitectureLevel::Mips3 => 3,
        Mips4ArchitectureLevel::Mips4 => 4,
    }
}

const fn disabled_action_code(
    action: se_device::cpu::mips4::instruction::requirements::Mips4DisabledInstructionAction,
) -> u64 {
    use se_device::cpu::mips4::instruction::requirements::Mips4DisabledInstructionAction;

    match action {
        Mips4DisabledInstructionAction::ReservedInstruction => 1,
        Mips4DisabledInstructionAction::FloatingPointUnimplemented => 2,
    }
}

const fn cp0_operation_code(operation: Mips4Cp0RuntimeOperation) -> u64 {
    match operation {
        Mips4Cp0RuntimeOperation::TransferFrom { .. } => 1,
        Mips4Cp0RuntimeOperation::TransferTo { .. } => 2,
        Mips4Cp0RuntimeOperation::TlbRead => 3,
        Mips4Cp0RuntimeOperation::TlbWriteIndexed => 4,
        Mips4Cp0RuntimeOperation::TlbWriteRandom => 5,
        Mips4Cp0RuntimeOperation::TlbProbe => 6,
        Mips4Cp0RuntimeOperation::Eret => 7,
        Mips4Cp0RuntimeOperation::Wait => 8,
        Mips4Cp0RuntimeOperation::Reserved => 9,
    }
}

#[repr(C)]
pub(super) struct Mips4NativeFrame {
    gpr: [u64; 32],
    gpr_write_through: *mut u64,
    hi: u64,
    lo: u64,
    pc: u64,
    next_pc: u64,
    delay_slot_branch_pc: u64,
    delay_slot_valid: u64,
    budget: u64,
    retired: u64,
    exception: u64,
    operations_executed: u64,
    runtime_calls: u64,
    operation_base: u64,
    region_side_exit: u64,
    runtime_context: *mut (),
    runtime_call: usize,
    fast_memory_context: *mut (),
    fast_memory_native_context: *mut Mips4FastMemoryContext,
    fast_memory_read: usize,
    fast_memory_read_start: u64,
    fast_memory_read_end: u64,
    runtime_value: u64,
    runtime_memory_big_endian: u64,
    runtime_operation_values: *const Mips4RuntimeOperation,
    runtime_operations: *const Mips4RuntimeOperationDescriptor,
    runtime_operation_count: u64,
}

impl Mips4NativeFrame {
    fn from_state(state: Mips4BlockFrameState) -> Self {
        Self {
            gpr: state.gpr,
            gpr_write_through: core::ptr::null_mut(),
            hi: state.hi,
            lo: state.lo,
            pc: state.pc,
            next_pc: state.next_pc,
            delay_slot_branch_pc: state.delay_slot_branch_pc.unwrap_or(0),
            delay_slot_valid: u64::from(state.delay_slot_branch_pc.is_some()),
            budget: state.budget,
            retired: state.retired,
            exception: state.exception.map_or(0, block_exception_code),
            operations_executed: state.operations_executed,
            runtime_calls: state.runtime_calls,
            operation_base: 0,
            region_side_exit: 0,
            runtime_context: core::ptr::null_mut(),
            runtime_call: 0,
            fast_memory_context: core::ptr::null_mut(),
            fast_memory_native_context: core::ptr::null_mut(),
            fast_memory_read: 0,
            fast_memory_read_start: 0,
            fast_memory_read_end: 0,
            runtime_value: 0,
            runtime_memory_big_endian: 0,
            runtime_operation_values: core::ptr::null(),
            runtime_operations: core::ptr::null(),
            runtime_operation_count: 0,
        }
    }

    fn export_state(&self) -> Mips4BlockFrameState {
        Mips4BlockFrameState {
            gpr: self.gpr,
            hi: self.hi,
            lo: self.lo,
            pc: self.pc,
            next_pc: self.next_pc,
            delay_slot_branch_pc: (self.delay_slot_valid != 0).then_some(self.delay_slot_branch_pc),
            budget: self.budget,
            retired: self.retired,
            exception: block_exception_from_code(self.exception),
            operations_executed: self.operations_executed,
            runtime_calls: self.runtime_calls,
        }
    }

    fn import_state(&mut self, state: Mips4BlockFrameState) {
        self.gpr = state.gpr;
        self.gpr[0] = 0;
        self.hi = state.hi;
        self.lo = state.lo;
        self.pc = state.pc;
        self.next_pc = state.next_pc;
        self.delay_slot_branch_pc = state.delay_slot_branch_pc.unwrap_or(0);
        self.delay_slot_valid = u64::from(state.delay_slot_branch_pc.is_some());
        self.budget = state.budget;
        self.retired = state.retired;
        self.exception = state.exception.map_or(0, block_exception_code);
        self.operations_executed = state.operations_executed;
        self.runtime_calls = state.runtime_calls;
    }
}

struct Mips4NativeRuntimeBinding<'call, 'object, R> {
    runtime: &'call mut R,
    operations: &'call [Mips4RuntimeOperation],
    fast_memory: Option<&'call mut (dyn Mips4NativeFastMemoryRuntime + 'object)>,
}

pub(super) struct Mips4NativeInvocation<'call, 'object, R> {
    frame: Mips4NativeFrame,
    semantic_frame: &'call mut Mips4BlockFrame,
    binding: Mips4NativeRuntimeBinding<'call, 'object, R>,
    descriptors: Vec<Mips4RuntimeOperationDescriptor>,
    native_context: *mut Mips4FastMemoryContext,
    read_start: u64,
    read_end: u64,
}

impl<'call, 'object, R> Mips4NativeInvocation<'call, 'object, R>
where
    R: Mips4BlockRuntime,
{
    pub(super) fn new(
        semantic_frame: &'call mut Mips4BlockFrame,
        runtime: &'call mut R,
        operations: &'call [Mips4RuntimeOperation],
        mut fast_memory: Option<&'call mut (dyn Mips4NativeFastMemoryRuntime + 'object)>,
    ) -> Self {
        let runtime_memory_big_endian = u64::from(runtime.runtime_memory_big_endian());
        let (read_start, read_end, native_context) = match fast_memory.as_mut() {
            Some(runtime) => {
                let (read_start, read_end) = runtime.native_read_physical_range().unwrap_or((0, 0));
                let native_context = runtime
                    .native_context()
                    .map_or(core::ptr::null_mut(), core::ptr::from_mut);
                (read_start, read_end, native_context)
            }
            None => (0, 0, core::ptr::null_mut()),
        };
        let descriptors = operations
            .iter()
            .copied()
            .map(Mips4RuntimeOperationDescriptor::from_operation)
            .collect();
        let mut frame = Mips4NativeFrame::from_state(semantic_frame.export_state());
        frame.runtime_memory_big_endian = runtime_memory_big_endian;
        Self {
            frame,
            semantic_frame,
            binding: Mips4NativeRuntimeBinding {
                runtime,
                operations,
                fast_memory,
            },
            descriptors,
            native_context,
            read_start,
            read_end,
        }
    }

    pub(super) fn frame_mut_ptr(&mut self) -> *mut Mips4NativeFrame {
        self.frame.gpr_write_through = self.frame.gpr.as_mut_ptr();
        self.frame.runtime_context = core::ptr::from_mut(&mut self.binding).cast();
        self.frame.runtime_call = mips4_runtime_trampoline::<R> as *const () as usize;
        self.frame.fast_memory_context = core::ptr::from_mut(&mut self.binding).cast();
        self.frame.fast_memory_native_context = self.native_context;
        self.frame.fast_memory_read =
            mips4_fast_memory_frame_read_trampoline::<R> as *const () as usize;
        self.frame.fast_memory_read_start = self.read_start;
        self.frame.fast_memory_read_end = self.read_end;
        self.frame.runtime_operation_values = self.binding.operations.as_ptr();
        self.frame.runtime_operations = self.descriptors.as_ptr();
        self.frame.runtime_operation_count = self.binding.operations.len() as u64;
        &mut self.frame
    }

    pub(super) fn region_side_exit(&self) -> Option<Mips4RegionSideExit> {
        region_side_exit_from_code(self.frame.region_side_exit)
    }

    pub(super) fn finish(self) {
        self.semantic_frame.import_state(self.frame.export_state());
    }
}

fn reborrow_native_fast_memory<'call, 'object>(
    runtime: &'call mut Option<&mut (dyn Mips4NativeFastMemoryRuntime + 'object)>,
) -> Option<&'call mut (dyn Mips4NativeFastMemoryRuntime + 'object)> {
    runtime.as_mut().map(|runtime| {
        let pointer: *mut (dyn Mips4NativeFastMemoryRuntime + 'object) = &mut **runtime;
        // SAFETY: The returned borrow is limited to the borrow of the option.
        unsafe { &mut *pointer }
    })
}

extern "C" fn mips4_runtime_trampoline<R>(
    context: *mut (),
    frame: *mut Mips4NativeFrame,
    operation: u32,
) -> u32
where
    R: Mips4BlockRuntime,
{
    catch_unwind(AssertUnwindSafe(|| {
        if context.is_null() || frame.is_null() {
            return runtime_result_code(Mips4RuntimeResult::InternalError);
        }
        // SAFETY: Native invocation owns the live binding and frame for this call.
        let binding = unsafe { &mut *context.cast::<Mips4NativeRuntimeBinding<'_, '_, R>>() };
        // SAFETY: The native entry uniquely borrows its frame during the call.
        let frame = unsafe { &mut *frame };
        let Some(operation) = binding.operations.get(operation as usize).copied() else {
            return runtime_result_code(Mips4RuntimeResult::InternalError);
        };
        let mut semantic_frame = Mips4BlockFrame::from_state(frame.export_state());
        let result = binding.runtime.execute(
            &mut semantic_frame,
            operation,
            reborrow_native_fast_memory(&mut binding.fast_memory),
        );
        frame.import_state(semantic_frame.export_state());
        runtime_result_code(result)
    }))
    .unwrap_or_else(|_| runtime_result_code(Mips4RuntimeResult::InternalError))
}

extern "C" fn mips4_fast_memory_frame_read_trampoline<R>(
    context: *mut (),
    frame: *mut Mips4NativeFrame,
    physical_address: u64,
    size: u32,
) -> u32
where
    R: Mips4BlockRuntime,
{
    catch_unwind(AssertUnwindSafe(|| {
        if context.is_null() || frame.is_null() {
            return 3;
        }
        // SAFETY: Native invocation owns the live binding and frame for this call.
        let binding = unsafe { &mut *context.cast::<Mips4NativeRuntimeBinding<'_, '_, R>>() };
        // SAFETY: The native entry uniquely borrows its frame during the call.
        let frame = unsafe { &mut *frame };
        let Some(runtime) = reborrow_native_fast_memory(&mut binding.fast_memory) else {
            return 0;
        };
        let size = match size {
            1 => Mips4ExecutionTransferSize::Byte,
            2 => Mips4ExecutionTransferSize::Halfword,
            4 => Mips4ExecutionTransferSize::Word,
            8 => Mips4ExecutionTransferSize::Doubleword,
            _ => return 3,
        };
        let request = Mips4FastMemoryReadRequest::new(
            physical_address,
            size,
            Mips4ExecutionAccessKind::DataLoad,
            Mips4MemoryAccessType::Uncached,
            frame.retired,
        );
        match runtime.read(request) {
            Mips4FastMemoryReadResult::Unavailable => 0,
            Mips4FastMemoryReadResult::Complete {
                value,
                retirement_limit,
            } => {
                let remaining = retirement_limit.saturating_sub(frame.retired);
                if remaining == 0 {
                    return 2;
                }
                frame.budget = frame.budget.min(remaining);
                frame.runtime_value = value;
                1
            }
            Mips4FastMemoryReadResult::TimelineExhausted => 2,
            Mips4FastMemoryReadResult::InternalError => 3,
        }
    }))
    .unwrap_or(3)
}

pub(super) const fn block_exit_code(exit: Mips4BlockExit) -> u32 {
    match exit {
        Mips4BlockExit::BudgetExhausted => 1,
        Mips4BlockExit::Dispatch => 2,
        Mips4BlockExit::Exception => 3,
        Mips4BlockExit::GuardInvalid => 4,
        Mips4BlockExit::RuntimeTransaction => 5,
        Mips4BlockExit::RuntimeIdle => 6,
        Mips4BlockExit::TimelineExhausted => 7,
        Mips4BlockExit::InternalError => 8,
    }
}

pub(super) const fn block_exit_from_code(code: u32) -> Option<Mips4BlockExit> {
    match code {
        1 => Some(Mips4BlockExit::BudgetExhausted),
        2 => Some(Mips4BlockExit::Dispatch),
        3 => Some(Mips4BlockExit::Exception),
        4 => Some(Mips4BlockExit::GuardInvalid),
        5 => Some(Mips4BlockExit::RuntimeTransaction),
        6 => Some(Mips4BlockExit::RuntimeIdle),
        7 => Some(Mips4BlockExit::TimelineExhausted),
        8 => Some(Mips4BlockExit::InternalError),
        _ => None,
    }
}

pub(super) const fn runtime_result_code(result: Mips4RuntimeResult) -> u32 {
    match result {
        Mips4RuntimeResult::Continue => 1,
        Mips4RuntimeResult::DispatchSequential => 2,
        Mips4RuntimeResult::DispatchControl => 3,
        Mips4RuntimeResult::Transaction => 4,
        Mips4RuntimeResult::Exception => 5,
        Mips4RuntimeResult::Idle => 6,
        Mips4RuntimeResult::InternalError => 7,
        Mips4RuntimeResult::ContinueControl => 8,
        Mips4RuntimeResult::TimelineExhausted => 9,
    }
}

pub(super) const fn block_exception_code(exception: Mips4BlockException) -> u64 {
    match exception {
        Mips4BlockException::ArithmeticOverflow => 1,
        Mips4BlockException::AddressErrorLoad => 2,
        Mips4BlockException::Trap => 3,
        Mips4BlockException::SystemCall => 4,
        Mips4BlockException::Breakpoint => 5,
    }
}

const fn block_exception_from_code(code: u64) -> Option<Mips4BlockException> {
    match code {
        1 => Some(Mips4BlockException::ArithmeticOverflow),
        2 => Some(Mips4BlockException::AddressErrorLoad),
        3 => Some(Mips4BlockException::Trap),
        4 => Some(Mips4BlockException::SystemCall),
        5 => Some(Mips4BlockException::Breakpoint),
        _ => None,
    }
}

pub(super) const fn region_side_exit_code(side_exit: Mips4RegionSideExit) -> u64 {
    match side_exit {
        Mips4RegionSideExit::ColdSuccessor => 1,
        Mips4RegionSideExit::Budget => 2,
        Mips4RegionSideExit::Runtime => 3,
        Mips4RegionSideExit::Guard => 4,
    }
}

pub(super) const fn region_side_exit_from_code(code: u64) -> Option<Mips4RegionSideExit> {
    match code {
        1 => Some(Mips4RegionSideExit::ColdSuccessor),
        2 => Some(Mips4RegionSideExit::Budget),
        3 => Some(Mips4RegionSideExit::Runtime),
        4 => Some(Mips4RegionSideExit::Guard),
        _ => None,
    }
}

pub(super) const MIPS4_BLOCK_FRAME_GPR_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeFrame, gpr) as i32;
pub(super) const MIPS4_BLOCK_FRAME_GPR_WRITE_THROUGH_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeFrame, gpr_write_through) as i32;
pub(super) const MIPS4_BLOCK_FRAME_HI_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeFrame, hi) as i32;
pub(super) const MIPS4_BLOCK_FRAME_LO_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeFrame, lo) as i32;
pub(super) const MIPS4_BLOCK_FRAME_PC_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeFrame, pc) as i32;
pub(super) const MIPS4_BLOCK_FRAME_NEXT_PC_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeFrame, next_pc) as i32;
pub(super) const MIPS4_BLOCK_FRAME_DELAY_PC_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeFrame, delay_slot_branch_pc) as i32;
pub(super) const MIPS4_BLOCK_FRAME_DELAY_VALID_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeFrame, delay_slot_valid) as i32;
pub(super) const MIPS4_BLOCK_FRAME_BUDGET_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeFrame, budget) as i32;
pub(super) const MIPS4_BLOCK_FRAME_RETIRED_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeFrame, retired) as i32;
pub(super) const MIPS4_BLOCK_FRAME_EXCEPTION_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeFrame, exception) as i32;
pub(super) const MIPS4_BLOCK_FRAME_OPERATIONS_EXECUTED_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeFrame, operations_executed) as i32;
pub(super) const MIPS4_BLOCK_FRAME_RUNTIME_CALLS_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeFrame, runtime_calls) as i32;
pub(super) const MIPS4_BLOCK_FRAME_OPERATION_BASE_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeFrame, operation_base) as i32;
pub(super) const MIPS4_BLOCK_FRAME_REGION_SIDE_EXIT_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeFrame, region_side_exit) as i32;
pub(super) const MIPS4_BLOCK_FRAME_RUNTIME_CONTEXT_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeFrame, runtime_context) as i32;
pub(super) const MIPS4_BLOCK_FRAME_RUNTIME_CALL_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeFrame, runtime_call) as i32;
pub(super) const MIPS4_BLOCK_FRAME_FAST_MEMORY_CONTEXT_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeFrame, fast_memory_context) as i32;
pub(super) const MIPS4_BLOCK_FRAME_FAST_MEMORY_NATIVE_CONTEXT_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeFrame, fast_memory_native_context) as i32;
pub(super) const MIPS4_BLOCK_FRAME_FAST_MEMORY_READ_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeFrame, fast_memory_read) as i32;
pub(super) const MIPS4_BLOCK_FRAME_FAST_MEMORY_READ_START_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeFrame, fast_memory_read_start) as i32;
pub(super) const MIPS4_BLOCK_FRAME_FAST_MEMORY_READ_END_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeFrame, fast_memory_read_end) as i32;
pub(super) const MIPS4_BLOCK_FRAME_RUNTIME_VALUE_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeFrame, runtime_value) as i32;
pub(super) const MIPS4_BLOCK_FRAME_RUNTIME_MEMORY_BIG_ENDIAN_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeFrame, runtime_memory_big_endian) as i32;

pub(super) const fn mips4_block_frame_gpr_offset(register: u8) -> i32 {
    MIPS4_BLOCK_FRAME_GPR_OFFSET + register as i32 * 8
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RejectRuntime;

    impl Mips4BlockRuntime for RejectRuntime {
        fn execute<F>(
            &mut self,
            _frame: &mut Mips4BlockFrame,
            _operation: Mips4RuntimeOperation,
            _fast_memory: Option<&mut F>,
        ) -> Mips4RuntimeResult
        where
            F: Mips4FastMemoryRuntime + ?Sized,
        {
            Mips4RuntimeResult::InternalError
        }
    }

    #[derive(Default)]
    struct FastMemory {
        completed: u64,
    }

    impl Mips4FastMemoryRuntime for FastMemory {
        fn read(&mut self, request: Mips4FastMemoryReadRequest) -> Mips4FastMemoryReadResult {
            assert_eq!(request.physical_address(), 0x1000);
            assert_eq!(request.retired_boundaries(), 2);
            self.completed += 1;
            Mips4FastMemoryReadResult::Complete {
                value: 0x0123_4567_89ab_cdef,
                retirement_limit: 6,
            }
        }

        fn completed_transactions(&self) -> u64 {
            self.completed
        }
    }

    impl Mips4NativeFastMemoryRuntime for FastMemory {
        fn native_read_physical_range(&self) -> Option<(u64, u64)> {
            Some((0x1000, 0x2000))
        }
    }

    #[test]
    fn semantic_state_round_trips_through_the_private_native_frame() {
        let mut gpr = [0; 32];
        gpr[1] = 11;
        let state = Mips4BlockFrameState {
            gpr,
            hi: 12,
            lo: 13,
            pc: 0x1000,
            next_pc: 0x1004,
            delay_slot_branch_pc: Some(0x0ffc),
            budget: 17,
            retired: 3,
            exception: Some(Mips4BlockException::Trap),
            operations_executed: 5,
            runtime_calls: 2,
        };
        let native = Mips4NativeFrame::from_state(state.clone());
        assert_eq!(native.export_state(), state);
    }

    #[test]
    fn fast_memory_trampoline_updates_native_budget_value_and_completion_count() {
        let mut semantic = Mips4BlockFrame::new([0; 32], 0, 0, 0x1000, 0x1004, None, 10);
        let mut state = semantic.export_state();
        state.retired = 2;
        semantic.import_state(state);
        let mut runtime = RejectRuntime;
        let mut fast_memory = FastMemory::default();
        let mut invocation =
            Mips4NativeInvocation::new(&mut semantic, &mut runtime, &[], Some(&mut fast_memory));
        let frame = invocation.frame_mut_ptr();
        // SAFETY: The invocation keeps the binding and native frame live for the call.
        let outcome = unsafe {
            mips4_fast_memory_frame_read_trampoline::<RejectRuntime>(
                (*frame).fast_memory_context,
                frame,
                0x1000,
                8,
            )
        };
        assert_eq!(outcome, 1);
        // SAFETY: The native frame remains uniquely owned by the invocation.
        unsafe {
            assert_eq!((*frame).budget, 4);
            assert_eq!((*frame).runtime_value, 0x0123_4567_89ab_cdef);
        }
        invocation.finish();
        assert_eq!(semantic.budget(), 4);
        assert_eq!(fast_memory.completed_transactions(), 1);
    }
}
