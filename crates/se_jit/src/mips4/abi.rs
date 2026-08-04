//! Private native ABI for MIPS IV host-code backends.

use std::panic::{AssertUnwindSafe, catch_unwind};

use se_device::cpu::mips4::execution::block::*;

use super::region::Mips4RegionSideExit;

#[repr(C)]
pub(super) struct Mips4NativeCallContext {
    operation_base: u64,
    region_side_exit: u64,
    runtime_context: *mut (),
    runtime_call: usize,
}

impl Mips4NativeCallContext {
    fn new() -> Self {
        Self {
            operation_base: 0,
            region_side_exit: 0,
            runtime_context: core::ptr::null_mut(),
            runtime_call: 0,
        }
    }
}

struct Mips4NativeRuntimeBinding<'call, R> {
    runtime: &'call mut R,
    operations: &'call [Mips4RuntimeOperation],
}

pub(super) struct Mips4NativeInvocation<'call, R> {
    context: Mips4NativeCallContext,
    binding: Mips4NativeRuntimeBinding<'call, R>,
}

impl<'call, R> Mips4NativeInvocation<'call, R>
where
    R: Mips4BlockRuntime,
{
    pub(super) fn new(runtime: &'call mut R, operations: &'call [Mips4RuntimeOperation]) -> Self {
        Self {
            context: Mips4NativeCallContext::new(),
            binding: Mips4NativeRuntimeBinding {
                runtime,
                operations,
            },
        }
    }

    pub(super) fn context_mut_ptr(&mut self) -> *mut Mips4NativeCallContext {
        self.context.runtime_context = core::ptr::from_mut(&mut self.binding).cast();
        self.context.runtime_call = mips4_runtime_trampoline::<R> as *const () as usize;
        &mut self.context
    }

    pub(super) fn region_side_exit(&self) -> Option<Mips4RegionSideExit> {
        region_side_exit_from_code(self.context.region_side_exit)
    }
}

extern "C" fn mips4_runtime_trampoline<R>(
    context: *mut (),
    frame: *mut Mips4BlockFrame,
    operation: u32,
) -> u32
where
    R: Mips4BlockRuntime,
{
    catch_unwind(AssertUnwindSafe(|| {
        if context.is_null() || frame.is_null() {
            return runtime_result_code(Mips4RuntimeResult::InternalError);
        }
        // SAFETY: The native invocation owns the live binding and frame for this call.
        let binding = unsafe { &mut *context.cast::<Mips4NativeRuntimeBinding<'_, R>>() };
        // SAFETY: The native entry uniquely borrows the canonical frame during the call.
        let frame = unsafe { &mut *frame };
        let Some(operation) = binding.operations.get(operation as usize).copied() else {
            return runtime_result_code(Mips4RuntimeResult::InternalError);
        };
        runtime_result_code(binding.runtime.execute(frame, operation))
    }))
    .unwrap_or_else(|_| runtime_result_code(Mips4RuntimeResult::InternalError))
}

pub(super) const fn block_exit_code(exit: Mips4BlockExit) -> u32 {
    match exit {
        Mips4BlockExit::BudgetExhausted => 1,
        Mips4BlockExit::Dispatch => 2,
        Mips4BlockExit::Exception => 3,
        Mips4BlockExit::GuardInvalid => 4,
        Mips4BlockExit::RuntimeTransaction => 5,
        Mips4BlockExit::RuntimeIdle => 6,
        Mips4BlockExit::InternalError => 7,
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
        7 => Some(Mips4BlockExit::InternalError),
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
pub(super) const fn mips4_block_frame_gpr_offset(register: u8) -> i32 {
    MIPS4_BLOCK_FRAME_GPR_OFFSET + register as i32 * 8
}

#[cfg(test)]
mod tests {
    use super::*;

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
        };
        let frame = Mips4BlockFrame::from_state(state.clone());
        assert_eq!(frame.export_state(), state);
        assert_eq!(
            block_exception_code(Mips4BlockException::Trap),
            Mips4BlockException::Trap as u64
        );
    }
}
