//! Private native ABI for MIPS IV host-code backends.

use std::panic::{AssertUnwindSafe, catch_unwind};

use se_device::cpu::mips4::cache::Mips4MemoryAccessType;
use se_device::cpu::mips4::execution::block::*;
use se_device::cpu::mips4::execution::bus::{Mips4ExecutionAccessKind, Mips4ExecutionTransferSize};

use super::region::Mips4RegionSideExit;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(C)]
pub(super) struct Mips4FastMemoryReadAbiResult {
    value: u64,
    retirement_limit: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(C)]
pub(super) struct Mips4NativeAffinePollAbiResult {
    iterations: u64,
    counter: u64,
    last_source: u64,
    remaining_budget: u64,
}

#[repr(C)]
pub(super) struct Mips4NativeCallContext {
    operation_base: u64,
    region_side_exit: u64,
    runtime_context: *mut (),
    runtime_call: usize,
    fast_memory_context: *mut (),
    fast_memory_read: usize,
    native_fast_memory_context: *mut Mips4NativeFastMemoryContext,
    native_affine_poll: usize,
    fast_memory_result: Mips4FastMemoryReadAbiResult,
    native_affine_poll_result: Mips4NativeAffinePollAbiResult,
    runtime_memory_big_endian: u64,
}

impl Mips4NativeCallContext {
    fn new(runtime_memory_big_endian: bool) -> Self {
        Self {
            operation_base: 0,
            region_side_exit: 0,
            runtime_context: core::ptr::null_mut(),
            runtime_call: 0,
            fast_memory_context: core::ptr::null_mut(),
            fast_memory_read: 0,
            native_fast_memory_context: core::ptr::null_mut(),
            native_affine_poll: mips4_native_affine_poll_trampoline as *const () as usize,
            fast_memory_result: Mips4FastMemoryReadAbiResult::default(),
            native_affine_poll_result: Mips4NativeAffinePollAbiResult::default(),
            runtime_memory_big_endian: u64::from(runtime_memory_big_endian),
        }
    }
}

extern "C" fn mips4_native_affine_poll_trampoline(
    context: *mut Mips4NativeFastMemoryContext,
    physical_address: u64,
    counter: u64,
    target: u64,
    retired: u64,
    budget: u64,
    result: *mut Mips4NativeAffinePollAbiResult,
) -> u32 {
    catch_unwind(AssertUnwindSafe(|| {
        if context.is_null() || result.is_null() {
            return 4;
        }
        // SAFETY: Native invocation owns the uniquely borrowed context during the call.
        let context = unsafe { &mut *context };
        let batch =
            context.execute_affine_poll_batch(physical_address, counter, target, retired, budget);
        if batch.disposition == Mips4NativeAffinePollDisposition::Unsupported {
            return 0;
        }
        // SAFETY: The native invocation supplied a live result slot.
        unsafe {
            *result = Mips4NativeAffinePollAbiResult {
                iterations: batch.iterations,
                counter: batch.counter,
                last_source: batch.last_source,
                remaining_budget: batch.remaining_budget,
            };
        }
        match batch.disposition {
            Mips4NativeAffinePollDisposition::Unsupported => 0,
            Mips4NativeAffinePollDisposition::Continue => 1,
            Mips4NativeAffinePollDisposition::BudgetExhausted => 2,
            Mips4NativeAffinePollDisposition::TimelineExhausted => 3,
        }
    }))
    .unwrap_or(4)
}

struct Mips4NativeRuntimeBinding<'call, 'object, R> {
    runtime: &'call mut R,
    operations: &'call [Mips4RuntimeOperation],
    fast_memory: Option<&'call mut (dyn Mips4FastMemoryRuntime + 'object)>,
}

pub(super) struct Mips4NativeInvocation<'call, 'object, R> {
    context: Mips4NativeCallContext,
    binding: Mips4NativeRuntimeBinding<'call, 'object, R>,
}

impl<'call, 'object, R> Mips4NativeInvocation<'call, 'object, R>
where
    R: Mips4BlockRuntime,
{
    pub(super) fn new(
        runtime: &'call mut R,
        operations: &'call [Mips4RuntimeOperation],
        fast_memory: Option<&'call mut (dyn Mips4FastMemoryRuntime + 'object)>,
    ) -> Self {
        Self {
            context: Mips4NativeCallContext::new(runtime.runtime_memory_big_endian()),
            binding: Mips4NativeRuntimeBinding {
                runtime,
                operations,
                fast_memory,
            },
        }
    }

    pub(super) fn context_mut_ptr(&mut self) -> *mut Mips4NativeCallContext {
        self.context.runtime_context = core::ptr::from_mut(&mut self.binding).cast();
        self.context.runtime_call = mips4_runtime_trampoline::<R> as *const () as usize;
        if self.binding.fast_memory.is_some() {
            self.context.fast_memory_context = core::ptr::from_mut(&mut self.binding).cast();
            self.context.fast_memory_read =
                mips4_fast_memory_read_trampoline::<R> as *const () as usize;
            self.context.native_fast_memory_context =
                reborrow_fast_memory(&mut self.binding.fast_memory)
                    .and_then(Mips4FastMemoryRuntime::native_context)
                    .map_or(core::ptr::null_mut(), core::ptr::from_mut);
        }
        &mut self.context
    }

    pub(super) fn region_side_exit(&self) -> Option<Mips4RegionSideExit> {
        region_side_exit_from_code(self.context.region_side_exit)
    }
}

fn reborrow_fast_memory<'call, 'object>(
    runtime: &'call mut Option<&mut (dyn Mips4FastMemoryRuntime + 'object)>,
) -> Option<&'call mut (dyn Mips4FastMemoryRuntime + 'object)> {
    runtime.as_mut().map(|runtime| {
        let pointer: *mut (dyn Mips4FastMemoryRuntime + 'object) = &mut **runtime;
        // SAFETY: The returned borrow is limited to the borrow of the option.
        unsafe { &mut *pointer }
    })
}

extern "C" fn mips4_runtime_trampoline<R>(
    context: *mut (),
    frame: *mut Mips4BlockFrame,
    operation: u32,
    allow_fast_memory: u32,
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
        // SAFETY: The native entry uniquely borrows the canonical frame during the call.
        let frame = unsafe { &mut *frame };
        let Some(operation) = binding.operations.get(operation as usize).copied() else {
            return runtime_result_code(Mips4RuntimeResult::InternalError);
        };
        let fast_memory = (allow_fast_memory != 0)
            .then(|| reborrow_fast_memory(&mut binding.fast_memory))
            .flatten();
        runtime_result_code(binding.runtime.execute(frame, operation, fast_memory))
    }))
    .unwrap_or_else(|_| runtime_result_code(Mips4RuntimeResult::InternalError))
}

extern "C" fn mips4_fast_memory_read_trampoline<R>(
    context: *mut (),
    physical_address: u64,
    retired_boundaries: u64,
    size: u32,
    result: *mut Mips4FastMemoryReadAbiResult,
) -> u32
where
    R: Mips4BlockRuntime,
{
    catch_unwind(AssertUnwindSafe(|| {
        if context.is_null() || result.is_null() {
            return 3;
        }
        // SAFETY: Native invocation owns the live binding and frame for this call.
        let binding = unsafe { &mut *context.cast::<Mips4NativeRuntimeBinding<'_, '_, R>>() };
        let Some(runtime) = reborrow_fast_memory(&mut binding.fast_memory) else {
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
            retired_boundaries,
        );
        match runtime.read(request) {
            Mips4FastMemoryReadResult::Unavailable => 0,
            Mips4FastMemoryReadResult::Complete {
                value,
                retirement_limit,
            } => {
                if retirement_limit <= retired_boundaries {
                    return 2;
                }
                // SAFETY: The native invocation supplied a live result slot.
                unsafe {
                    *result = Mips4FastMemoryReadAbiResult {
                        value,
                        retirement_limit,
                    };
                }
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
    exception as u64
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

pub(super) const MIPS4_NATIVE_CALL_OPERATION_BASE_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeCallContext, operation_base) as i32;
pub(super) const MIPS4_NATIVE_CALL_REGION_SIDE_EXIT_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeCallContext, region_side_exit) as i32;
pub(super) const MIPS4_NATIVE_CALL_RUNTIME_CONTEXT_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeCallContext, runtime_context) as i32;
pub(super) const MIPS4_NATIVE_CALL_RUNTIME_CALL_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeCallContext, runtime_call) as i32;
pub(super) const MIPS4_NATIVE_CALL_FAST_MEMORY_CONTEXT_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeCallContext, fast_memory_context) as i32;
pub(super) const MIPS4_NATIVE_CALL_FAST_MEMORY_READ_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeCallContext, fast_memory_read) as i32;
pub(super) const MIPS4_NATIVE_CALL_NATIVE_FAST_MEMORY_CONTEXT_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeCallContext, native_fast_memory_context) as i32;
pub(super) const MIPS4_NATIVE_CALL_NATIVE_AFFINE_POLL_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeCallContext, native_affine_poll) as i32;
pub(super) const MIPS4_NATIVE_CALL_FAST_MEMORY_RESULT_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeCallContext, fast_memory_result) as i32;
pub(super) const MIPS4_NATIVE_CALL_NATIVE_AFFINE_POLL_RESULT_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeCallContext, native_affine_poll_result) as i32;
pub(super) const MIPS4_FAST_MEMORY_RESULT_VALUE_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryReadAbiResult, value) as i32;
pub(super) const MIPS4_FAST_MEMORY_RESULT_RETIREMENT_LIMIT_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryReadAbiResult, retirement_limit) as i32;
pub(super) const MIPS4_NATIVE_AFFINE_POLL_ITERATIONS_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeAffinePollAbiResult, iterations) as i32;
pub(super) const MIPS4_NATIVE_AFFINE_POLL_COUNTER_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeAffinePollAbiResult, counter) as i32;
pub(super) const MIPS4_NATIVE_AFFINE_POLL_LAST_SOURCE_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeAffinePollAbiResult, last_source) as i32;
pub(super) const MIPS4_NATIVE_AFFINE_POLL_REMAINING_BUDGET_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeAffinePollAbiResult, remaining_budget) as i32;
pub(super) const MIPS4_NATIVE_CALL_RUNTIME_MEMORY_BIG_ENDIAN_OFFSET: i32 =
    core::mem::offset_of!(Mips4NativeCallContext, runtime_memory_big_endian) as i32;

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

    #[derive(Clone, Copy)]
    enum FastMemoryFailure {
        Unavailable,
        TimelineExhausted,
        InternalError,
        Panic,
    }

    struct FailingFastMemory(FastMemoryFailure);

    impl Mips4FastMemoryRuntime for FailingFastMemory {
        fn read(&mut self, _request: Mips4FastMemoryReadRequest) -> Mips4FastMemoryReadResult {
            match self.0 {
                FastMemoryFailure::Unavailable => Mips4FastMemoryReadResult::Unavailable,
                FastMemoryFailure::TimelineExhausted => {
                    Mips4FastMemoryReadResult::TimelineExhausted
                }
                FastMemoryFailure::InternalError => Mips4FastMemoryReadResult::InternalError,
                FastMemoryFailure::Panic => panic!("fast-memory test panic"),
            }
        }
    }

    #[test]
    fn canonical_frame_preserves_portable_state_and_abi_codes() {
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
            native_fast_memory_reads: 7,
        };
        let frame = Mips4BlockFrame::from_state(state.clone());
        assert_eq!(frame.export_state(), state);
        assert_eq!(
            block_exception_code(Mips4BlockException::Trap),
            Mips4BlockException::Trap as u64
        );
    }

    #[test]
    fn fast_memory_trampoline_returns_value_limit_and_completion_count() {
        let mut semantic = Mips4BlockFrame::new([0; 32], 0, 0, 0x1000, 0x1004, None, 10);
        let mut state = semantic.export_state();
        state.retired = 2;
        semantic.import_state(state);
        let mut runtime = RejectRuntime;
        let mut fast_memory = FastMemory::default();
        let mut invocation = Mips4NativeInvocation::new(&mut runtime, &[], Some(&mut fast_memory));
        let context = invocation.context_mut_ptr();
        // SAFETY: The invocation keeps the binding and call context live for the call.
        let outcome = unsafe {
            mips4_fast_memory_read_trampoline::<RejectRuntime>(
                (*context).fast_memory_context,
                0x1000,
                2,
                8,
                &mut (*context).fast_memory_result,
            )
        };
        assert_eq!(outcome, 1);
        // SAFETY: The call context remains uniquely owned by the invocation.
        unsafe {
            assert_eq!(
                (*context).fast_memory_result,
                Mips4FastMemoryReadAbiResult {
                    value: 0x0123_4567_89ab_cdef,
                    retirement_limit: 6,
                }
            );
        }
        assert_eq!(semantic.budget(), 10);
        assert_eq!(fast_memory.completed_transactions(), 1);
    }

    #[test]
    fn fast_memory_trampoline_maps_failures_and_contains_panics() {
        for (failure, expected) in [
            (FastMemoryFailure::Unavailable, 0),
            (FastMemoryFailure::TimelineExhausted, 2),
            (FastMemoryFailure::InternalError, 3),
            (FastMemoryFailure::Panic, 3),
        ] {
            let semantic = Mips4BlockFrame::new([0; 32], 0, 0, 0x1000, 0x1004, None, 1);
            let mut runtime = RejectRuntime;
            let mut fast_memory = FailingFastMemory(failure);
            let mut invocation =
                Mips4NativeInvocation::new(&mut runtime, &[], Some(&mut fast_memory));
            let context = invocation.context_mut_ptr();
            // SAFETY: The invocation keeps the binding and result slot live for the call.
            let outcome = unsafe {
                mips4_fast_memory_read_trampoline::<RejectRuntime>(
                    (*context).fast_memory_context,
                    0x1000,
                    0,
                    8,
                    &mut (*context).fast_memory_result,
                )
            };
            assert_eq!(outcome, expected);
            assert_eq!(semantic.budget(), 1);
        }
    }
}
