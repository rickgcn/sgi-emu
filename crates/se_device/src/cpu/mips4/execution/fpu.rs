//! Functional CP1 instruction semantics and memory requests.

use se_float::backend::FloatBackend;
use se_float::control::{FloatControl, FloatExceptionFlags, FloatRoundingMode};
use se_float::result::FloatResult;
use se_float::value::{
    Float32Bits, Float64Bits, FloatClass, FloatCompareMode, FloatNanMode, FloatRelation,
};

use crate::cpu::mips4::branch::{Mips4Branch, Mips4BranchDecision};
use crate::cpu::mips4::cache::hierarchy::Mips4CacheAccessPolicy;
use crate::cpu::mips4::config::Mips4Endianness;
use crate::cpu::mips4::cp1::decode::{
    Mips4Cp1BranchOperation, Mips4Cp1CompareCondition, Mips4Cp1Decode,
    Mips4Cp1IndexedMemoryOperation, Mips4Cp1InstructionClass, Mips4Cp1MovciOperation,
    Mips4Cp1OffsetMemoryOperation, Mips4Cp1OperandFormatStatus, Mips4Cp1Operation,
    Mips4Cp1RegisterTransferOperation,
};
use crate::cpu::mips4::cp1::operation::{
    Mips4Cp1IndexedMemoryAccess, Mips4Cp1IndexedPrefetch, Mips4Cp1OffsetMemoryAccess,
};
use crate::cpu::mips4::cp1::{
    MIPS4_COP1X_OPCODE, Mips4Cp1ConditionCode, Mips4Cp1ControlRegister,
    Mips4Cp1ConversionRoundingMode, Mips4Cp1FgrIndex, Mips4Cp1Format, Mips4Cp1Instruction,
    Mips4Cp1MoveDecision, Mips4Cp1RegisterMode,
};
use crate::cpu::mips4::exception::{Mips4CoprocessorNumber, Mips4Exception};
use crate::cpu::mips4::gpr::{Mips4GprIndex, sign_extend_word};
use crate::cpu::mips4::instruction::Mips4Instruction;
use crate::cpu::mips4::instruction::requirements::cp1_requirements;
use crate::cpu::mips4::memory::operation::{
    Mips4MemoryAccessError, Mips4Prefetch, Mips4PrefetchHint, Mips4PrefetchResult,
};
use crate::cpu::mips4::mmu::Mips4MmuCacheAttribute;
use crate::cpu::mips4::tlb::Mips4TlbAsid;

use super::access::{
    Mips4InstructionAccess, check_architecture_level, coprocessor_unusable, reserved,
};
use super::bus::{Mips4ExecutionAccessKind, Mips4ExecutionTransaction, Mips4ExecutionTransferSize};
use super::memory::{decode_u32, decode_u64, encode_lanes};
use super::policy::{Mips4ExecutionPolicy, Mips4PrefetchPolicy, Mips4ReservedCp1ControlPolicy};
use super::state::Mips4ExecutionState;

pub(super) enum Mips4FpuExecution {
    Retire,
    Branch(Mips4BranchDecision),
    Read {
        pending: Mips4PendingFpuRead,
        transaction: Mips4ExecutionTransaction,
        virtual_address: u64,
        cache_policy: Mips4CacheAccessPolicy,
    },
    Write {
        transaction: Mips4ExecutionTransaction,
        virtual_address: u64,
        cache_policy: Mips4CacheAccessPolicy,
    },
    Prefetch(Mips4Prefetch),
    Exception(Mips4Exception),
}

#[derive(Clone, Copy, serde::Deserialize, serde::Serialize)]
pub(super) struct Mips4PendingFpuRead {
    target: Mips4Cp1FgrIndex,
    size: Mips4ExecutionTransferSize,
    register_mode: Mips4Cp1RegisterMode,
}

pub(super) fn execute_fpu<F: FloatBackend>(
    state: &mut Mips4ExecutionState,
    backend: &F,
    policy: &impl Mips4ExecutionPolicy,
    raw: Mips4Instruction,
    decoded: Mips4Cp1Decode,
    endianness: Mips4Endianness,
) -> Result<Mips4FpuExecution, Mips4MemoryAccessError> {
    let access = check_fpu_access(
        state.cp0.status(),
        state.config.coprocessors.cp1,
        raw,
        decoded,
    );
    match access {
        Mips4InstructionAccess::Execute => {}
        Mips4InstructionAccess::Exception(exception) => {
            return Ok(Mips4FpuExecution::Exception(exception));
        }
        Mips4InstructionAccess::FloatingPointUnimplemented => {
            return Ok(unimplemented(state));
        }
    }
    let Mips4Cp1Decode::Instruction(class) = decoded else {
        unreachable!();
    };
    match class {
        Mips4Cp1InstructionClass::RegisterTransfer(operation) => {
            Ok(execute_transfer(state, policy, raw, operation))
        }
        Mips4Cp1InstructionClass::Branch(operation) => Ok(execute_branch(state, raw, operation)),
        Mips4Cp1InstructionClass::Formatted { operation, format } => {
            Ok(execute_formatted(state, backend, raw, operation, format))
        }
        Mips4Cp1InstructionClass::Movci(operation) => Ok(execute_movci(state, raw, operation)),
        Mips4Cp1InstructionClass::OffsetMemory(operation) => {
            prepare_offset_memory(state, policy, raw, operation, endianness)
        }
        Mips4Cp1InstructionClass::IndexedMemory(operation) => {
            prepare_indexed_memory(state, policy, raw, operation, endianness)
        }
        Mips4Cp1InstructionClass::IndexedPrefetch => {
            if matches!(policy.prefetch_policy(), Mips4PrefetchPolicy::NoOperation) {
                return Ok(Mips4FpuExecution::Retire);
            }
            let instruction = Mips4Cp1Instruction::from_instruction(raw).unwrap();
            let hint = Mips4PrefetchHint::from_bits(instruction.prefetch_hint());
            if !hint.is_defined() {
                return Ok(Mips4FpuExecution::Retire);
            }
            let base = read_gpr(state, instruction.base());
            let index = read_gpr(state, instruction.index());
            let virtual_address = base.wrapping_add(index);
            let tlb_entries = state.deterministic_tlb_entries(policy, virtual_address);
            match Mips4Cp1IndexedPrefetch::prepare(
                base,
                index,
                hint,
                policy.mmu_config(state.cp0.config()),
                state.cp0.status(),
                Mips4TlbAsid::new(state.cp0.entry_hi().address_space_identifier()),
                tlb_entries,
            ) {
                Mips4PrefetchResult::Request(prefetch) => Ok(Mips4FpuExecution::Prefetch(prefetch)),
                Mips4PrefetchResult::NoOperation => Ok(Mips4FpuExecution::Retire),
            }
        }
    }
}

pub(super) fn check_fpu_access(
    status: crate::cpu::mips4::cp0::Mips4Cp0Status,
    present: bool,
    raw: Mips4Instruction,
    decoded: Mips4Cp1Decode,
) -> Mips4InstructionAccess {
    let usable = status.coprocessor_usable(Mips4CoprocessorNumber::Cp1);
    let cop1x = raw.opcode() == MIPS4_COP1X_OPCODE;

    if cop1x {
        let requirements =
            crate::cpu::mips4::instruction::requirements::Mips4InstructionRequirements::new(
                crate::cpu::mips4::instruction::requirements::Mips4ArchitectureLevel::Mips4,
            );
        if !matches!(
            check_architecture_level(status, requirements),
            Mips4InstructionAccess::Execute
        ) {
            return reserved();
        }
        if !usable {
            return coprocessor_unusable(Mips4CoprocessorNumber::Cp1);
        }
        if !present || matches!(decoded, Mips4Cp1Decode::ReservedOrUnimplementedOperation) {
            return reserved();
        }
        return Mips4InstructionAccess::Execute;
    }

    let Mips4Cp1Decode::Instruction(class) = decoded else {
        return if !usable {
            coprocessor_unusable(Mips4CoprocessorNumber::Cp1)
        } else if !present {
            reserved()
        } else {
            Mips4InstructionAccess::FloatingPointUnimplemented
        };
    };
    let requirements = cp1_requirements(raw, class);

    if matches!(class, Mips4Cp1InstructionClass::Movci(_)) {
        if !matches!(
            check_architecture_level(status, requirements),
            Mips4InstructionAccess::Execute
        ) {
            return reserved();
        }
        if !usable {
            return coprocessor_unusable(Mips4CoprocessorNumber::Cp1);
        }
        return if present {
            Mips4InstructionAccess::Execute
        } else {
            reserved()
        };
    }

    if matches!(class, Mips4Cp1InstructionClass::Branch(_))
        && !matches!(
            check_architecture_level(status, requirements),
            Mips4InstructionAccess::Execute
        )
    {
        return reserved();
    }
    if !usable {
        return coprocessor_unusable(Mips4CoprocessorNumber::Cp1);
    }
    if !present {
        return reserved();
    }
    check_architecture_level(status, requirements)
}

pub(super) fn complete_fpu_read(
    state: &mut Mips4ExecutionState,
    pending: Mips4PendingFpuRead,
    lanes: u64,
    endianness: Mips4Endianness,
) -> Mips4FpuExecution {
    match pending.size {
        Mips4ExecutionTransferSize::Word => state
            .cp1
            .fgr_mut()
            .write_word(pending.target, decode_u32(lanes, endianness)),
        Mips4ExecutionTransferSize::Doubleword => {
            if state
                .cp1
                .fgr_mut()
                .write_doubleword(
                    pending.register_mode,
                    pending.target,
                    decode_u64(lanes, endianness),
                )
                .is_err()
            {
                return unimplemented(state);
            }
        }
        Mips4ExecutionTransferSize::Byte | Mips4ExecutionTransferSize::Halfword => unreachable!(),
    }
    Mips4FpuExecution::Retire
}

fn execute_transfer(
    state: &mut Mips4ExecutionState,
    policy: &impl Mips4ExecutionPolicy,
    raw: Mips4Instruction,
    operation: Mips4Cp1RegisterTransferOperation,
) -> Mips4FpuExecution {
    if raw.bits() & 0x7ff != 0 {
        return unimplemented(state);
    }
    let fgr = fgr(raw.rd());
    let mode = register_mode(state);
    match operation {
        Mips4Cp1RegisterTransferOperation::MoveWordFrom => {
            let value = sign_extend_word(state.cp1.fgr().read_word(fgr));
            write_gpr(state, raw.rt(), value);
        }
        Mips4Cp1RegisterTransferOperation::MoveDoublewordFrom => {
            let Ok(value) = state.cp1.fgr().read_doubleword(mode, fgr) else {
                return unimplemented(state);
            };
            write_gpr(state, raw.rt(), value);
        }
        Mips4Cp1RegisterTransferOperation::MoveControlFrom => {
            let value = match Mips4Cp1ControlRegister::from_u8(raw.rd()) {
                Some(register) => state.cp1.read_control(register),
                None => match policy.reserved_cp1_control_policy(raw.rd()) {
                    Mips4ReservedCp1ControlPolicy::ReadZeroWriteIgnore => 0,
                    Mips4ReservedCp1ControlPolicy::FloatingPointUnimplemented => {
                        return unimplemented(state);
                    }
                },
            };
            write_gpr(state, raw.rt(), sign_extend_word(value));
        }
        Mips4Cp1RegisterTransferOperation::MoveWordTo => {
            let value = read_gpr(state, raw.rt()) as u32;
            state.cp1.fgr_mut().write_word(fgr, value);
        }
        Mips4Cp1RegisterTransferOperation::MoveDoublewordTo => {
            let value = read_gpr(state, raw.rt());
            if state
                .cp1
                .fgr_mut()
                .write_doubleword(mode, fgr, value)
                .is_err()
            {
                return unimplemented(state);
            }
        }
        Mips4Cp1RegisterTransferOperation::MoveControlTo => {
            let register = match Mips4Cp1ControlRegister::from_u8(raw.rd()) {
                Some(register) => register,
                None => match policy.reserved_cp1_control_policy(raw.rd()) {
                    Mips4ReservedCp1ControlPolicy::ReadZeroWriteIgnore => {
                        return Mips4FpuExecution::Retire;
                    }
                    Mips4ReservedCp1ControlPolicy::FloatingPointUnimplemented => {
                        return unimplemented(state);
                    }
                },
            };
            let value = read_gpr(state, raw.rt()) as u32;
            let _ = state.cp1.write_control(register, value);
            if matches!(register, Mips4Cp1ControlRegister::ControlStatus) {
                let fcsr = state.cp1.fcsr();
                if fcsr.unimplemented_operation_cause()
                    || fcsr.cause_flags().bits() & fcsr.enable_flags().bits() != 0
                {
                    return Mips4FpuExecution::Exception(Mips4Exception::FloatingPoint);
                }
            }
        }
    }
    Mips4FpuExecution::Retire
}

fn execute_branch(
    state: &Mips4ExecutionState,
    raw: Mips4Instruction,
    operation: Mips4Cp1BranchOperation,
) -> Mips4FpuExecution {
    let instruction = Mips4Cp1Instruction::from_instruction(raw).unwrap();
    let condition = condition_code(instruction.branch_condition_code_bits());
    let value = state.cp1.fcsr().condition_code(condition);
    let decision = match operation {
        Mips4Cp1BranchOperation::BranchFalse => {
            Mips4Branch::bc1f(state.pc, value, raw.signed_immediate())
        }
        Mips4Cp1BranchOperation::BranchTrue => {
            Mips4Branch::bc1t(state.pc, value, raw.signed_immediate())
        }
        Mips4Cp1BranchOperation::BranchFalseLikely => {
            Mips4Branch::bc1fl(state.pc, value, raw.signed_immediate())
        }
        Mips4Cp1BranchOperation::BranchTrueLikely => {
            Mips4Branch::bc1tl(state.pc, value, raw.signed_immediate())
        }
    };
    Mips4FpuExecution::Branch(decision)
}

fn execute_movci(
    state: &mut Mips4ExecutionState,
    raw: Mips4Instruction,
    operation: Mips4Cp1MovciOperation,
) -> Mips4FpuExecution {
    let fcc = state
        .cp1
        .fcsr()
        .condition_code(condition_code((raw.rt() >> 2) & 0x07));
    let should_move = match operation {
        Mips4Cp1MovciOperation::MoveFalse => !fcc,
        Mips4Cp1MovciOperation::MoveTrue => fcc,
    };
    if should_move {
        write_gpr(state, raw.rd(), read_gpr(state, raw.rs()));
    }
    Mips4FpuExecution::Retire
}

fn execute_formatted<F: FloatBackend>(
    state: &mut Mips4ExecutionState,
    backend: &F,
    raw: Mips4Instruction,
    operation: Mips4Cp1Operation,
    format: Mips4Cp1Format,
) -> Mips4FpuExecution {
    match operation.operand_format_status(format) {
        Mips4Cp1OperandFormatStatus::Valid => {}
        Mips4Cp1OperandFormatStatus::UnimplementedOrReserved
        | Mips4Cp1OperandFormatStatus::Invalid => return unimplemented(state),
    }

    match operation {
        Mips4Cp1Operation::Move
        | Mips4Cp1Operation::MoveConditionalFalse
        | Mips4Cp1Operation::MoveConditionalTrue
        | Mips4Cp1Operation::MoveConditionalNonzero
        | Mips4Cp1Operation::MoveConditionalZero => {
            return execute_formatted_move(state, raw, operation, format);
        }
        Mips4Cp1Operation::Compare(condition) => {
            return execute_compare(state, backend, raw, format, condition);
        }
        _ => {}
    }

    match format {
        Mips4Cp1Format::Single => execute_single(state, backend, raw, operation),
        Mips4Cp1Format::Double => execute_double(state, backend, raw, operation),
        Mips4Cp1Format::Word | Mips4Cp1Format::Long => {
            execute_fixed_conversion(state, backend, raw, operation, format)
        }
    }
}

fn execute_formatted_move(
    state: &mut Mips4ExecutionState,
    raw: Mips4Instruction,
    operation: Mips4Cp1Operation,
    format: Mips4Cp1Format,
) -> Mips4FpuExecution {
    let should_move = match operation {
        Mips4Cp1Operation::Move => true,
        Mips4Cp1Operation::MoveConditionalFalse | Mips4Cp1Operation::MoveConditionalTrue => {
            let fcc = state
                .cp1
                .fcsr()
                .condition_code(condition_code((raw.ft() >> 2) & 0x07));
            if matches!(operation, Mips4Cp1Operation::MoveConditionalFalse) {
                Mips4Cp1MoveDecision::move_conditional_false(fcc).is_move()
            } else {
                Mips4Cp1MoveDecision::move_conditional_true(fcc).is_move()
            }
        }
        Mips4Cp1Operation::MoveConditionalNonzero => {
            Mips4Cp1MoveDecision::move_conditional_nonzero(read_gpr(state, raw.rt())).is_move()
        }
        Mips4Cp1Operation::MoveConditionalZero => {
            Mips4Cp1MoveDecision::move_conditional_zero(read_gpr(state, raw.rt())).is_move()
        }
        _ => unreachable!(),
    };
    if !should_move {
        return Mips4FpuExecution::Retire;
    }
    let mode = register_mode(state);
    let Some(source) = read_formatted(state, mode, fgr(raw.fs()), format) else {
        return unimplemented(state);
    };
    if write_formatted(state, mode, fgr(raw.fd()), format, source).is_err() {
        return unimplemented(state);
    }
    Mips4FpuExecution::Retire
}

fn execute_single<F: FloatBackend>(
    state: &mut Mips4ExecutionState,
    backend: &F,
    raw: Mips4Instruction,
    operation: Mips4Cp1Operation,
) -> Mips4FpuExecution {
    if is_conversion_to_integer(operation) {
        return convert_single_to_integer(state, backend, raw, operation);
    }
    if matches!(operation, Mips4Cp1Operation::ConvertDouble) {
        let source = Float32Bits::new(state.cp1.fgr().read_word(fgr(raw.fs())));
        if unsupported_float_class(source.classify(FloatNanMode::QuietBitSet)) {
            return unimplemented(state);
        }
        let result = backend.f32_to_f64(state.cp1.fcsr().float_control(), source);
        return commit_f64(state, raw.fd(), result);
    }

    let fs = Float32Bits::new(state.cp1.fgr().read_word(fgr(raw.fs())));
    let ft = Float32Bits::new(state.cp1.fgr().read_word(fgr(raw.ft())));
    let fr = Float32Bits::new(state.cp1.fgr().read_word(fgr(raw.rs())));
    if operation_uses_unsupported_inputs(
        operation,
        fs.classify(FloatNanMode::QuietBitSet),
        ft.classify(FloatNanMode::QuietBitSet),
        fr.classify(FloatNanMode::QuietBitSet),
    ) {
        return unimplemented(state);
    }
    let control = state.cp1.fcsr().float_control();
    let result = match operation {
        Mips4Cp1Operation::Add => backend.add_f32(control, fs, ft),
        Mips4Cp1Operation::Subtract => backend.sub_f32(control, fs, ft),
        Mips4Cp1Operation::Multiply => backend.mul_f32(control, fs, ft),
        Mips4Cp1Operation::Divide => backend.div_f32(control, fs, ft),
        Mips4Cp1Operation::SquareRoot => backend.sqrt_f32(control, fs),
        Mips4Cp1Operation::Reciprocal => {
            backend.div_f32(control, Float32Bits::new(1.0f32.to_bits()), fs)
        }
        Mips4Cp1Operation::ReciprocalSquareRoot => {
            let root = backend.sqrt_f32(control, fs);
            let reciprocal =
                backend.div_f32(control, Float32Bits::new(1.0f32.to_bits()), root.value);
            FloatResult::new(reciprocal.value, root.flags | reciprocal.flags)
        }
        Mips4Cp1Operation::Absolute => unary_f32(fs, false),
        Mips4Cp1Operation::Negate => unary_f32(fs, true),
        Mips4Cp1Operation::MultiplyAdd
        | Mips4Cp1Operation::MultiplySubtract
        | Mips4Cp1Operation::NegativeMultiplyAdd
        | Mips4Cp1Operation::NegativeMultiplySubtract => {
            let Some(result) = multiply_accumulate_f32(backend, control, operation, fs, ft, fr)
            else {
                return unimplemented(state);
            };
            result
        }
        _ => return unimplemented(state),
    };
    commit_f32(state, raw.fd(), result)
}

fn execute_double<F: FloatBackend>(
    state: &mut Mips4ExecutionState,
    backend: &F,
    raw: Mips4Instruction,
    operation: Mips4Cp1Operation,
) -> Mips4FpuExecution {
    if is_conversion_to_integer(operation) {
        return convert_double_to_integer(state, backend, raw, operation);
    }
    let mode = register_mode(state);
    let Some(fs_bits) = read_doubleword(state, mode, raw.fs()) else {
        return unimplemented(state);
    };
    if matches!(operation, Mips4Cp1Operation::ConvertSingle) {
        let source = Float64Bits::new(fs_bits);
        if unsupported_float_class(source.classify(FloatNanMode::QuietBitSet)) {
            return unimplemented(state);
        }
        let result = backend.f64_to_f32(state.cp1.fcsr().float_control(), source);
        return commit_f32(state, raw.fd(), result);
    }
    let Some(ft_bits) = read_doubleword(state, mode, raw.ft()) else {
        return unimplemented(state);
    };
    let fs = Float64Bits::new(fs_bits);
    let ft = Float64Bits::new(ft_bits);
    let fr = if is_multiply_accumulate(operation) {
        let Some(bits) = read_doubleword(state, mode, raw.rs()) else {
            return unimplemented(state);
        };
        Float64Bits::new(bits)
    } else {
        Float64Bits::new(0)
    };
    if operation_uses_unsupported_inputs(
        operation,
        fs.classify(FloatNanMode::QuietBitSet),
        ft.classify(FloatNanMode::QuietBitSet),
        fr.classify(FloatNanMode::QuietBitSet),
    ) {
        return unimplemented(state);
    }
    let control = state.cp1.fcsr().float_control();
    let result = match operation {
        Mips4Cp1Operation::Add => backend.add_f64(control, fs, ft),
        Mips4Cp1Operation::Subtract => backend.sub_f64(control, fs, ft),
        Mips4Cp1Operation::Multiply => backend.mul_f64(control, fs, ft),
        Mips4Cp1Operation::Divide => backend.div_f64(control, fs, ft),
        Mips4Cp1Operation::SquareRoot => backend.sqrt_f64(control, fs),
        Mips4Cp1Operation::Reciprocal => {
            backend.div_f64(control, Float64Bits::new(1.0f64.to_bits()), fs)
        }
        Mips4Cp1Operation::ReciprocalSquareRoot => {
            let root = backend.sqrt_f64(control, fs);
            let reciprocal =
                backend.div_f64(control, Float64Bits::new(1.0f64.to_bits()), root.value);
            FloatResult::new(reciprocal.value, root.flags | reciprocal.flags)
        }
        Mips4Cp1Operation::Absolute => unary_f64(fs, false),
        Mips4Cp1Operation::Negate => unary_f64(fs, true),
        Mips4Cp1Operation::MultiplyAdd
        | Mips4Cp1Operation::MultiplySubtract
        | Mips4Cp1Operation::NegativeMultiplyAdd
        | Mips4Cp1Operation::NegativeMultiplySubtract => {
            let Some(result) = multiply_accumulate_f64(backend, control, operation, fs, ft, fr)
            else {
                return unimplemented(state);
            };
            result
        }
        _ => return unimplemented(state),
    };
    commit_f64(state, raw.fd(), result)
}

fn execute_fixed_conversion<F: FloatBackend>(
    state: &mut Mips4ExecutionState,
    backend: &F,
    raw: Mips4Instruction,
    operation: Mips4Cp1Operation,
    format: Mips4Cp1Format,
) -> Mips4FpuExecution {
    let mode = register_mode(state);
    let control = state.cp1.fcsr().float_control();
    match (operation, format) {
        (Mips4Cp1Operation::ConvertSingle, Mips4Cp1Format::Word) => {
            let value = state.cp1.fgr().read_word(fgr(raw.fs())) as i32;
            commit_f32(state, raw.fd(), backend.i32_to_f32(control, value))
        }
        (Mips4Cp1Operation::ConvertDouble, Mips4Cp1Format::Word) => {
            let value = state.cp1.fgr().read_word(fgr(raw.fs())) as i32;
            commit_f64(state, raw.fd(), backend.i32_to_f64(control, value))
        }
        (Mips4Cp1Operation::ConvertSingle, Mips4Cp1Format::Long)
        | (Mips4Cp1Operation::ConvertDouble, Mips4Cp1Format::Long) => {
            let Some(value) = read_doubleword(state, mode, raw.fs()).map(|value| value as i64)
            else {
                return unimplemented(state);
            };
            if !(-(1_i64 << 52)..(1_i64 << 52)).contains(&value) {
                return unimplemented(state);
            }
            if matches!(operation, Mips4Cp1Operation::ConvertSingle) {
                commit_f32(state, raw.fd(), backend.i64_to_f32(control, value))
            } else {
                commit_f64(state, raw.fd(), backend.i64_to_f64(control, value))
            }
        }
        _ => unimplemented(state),
    }
}

fn convert_single_to_integer<F: FloatBackend>(
    state: &mut Mips4ExecutionState,
    backend: &F,
    raw: Mips4Instruction,
    operation: Mips4Cp1Operation,
) -> Mips4FpuExecution {
    let source = Float32Bits::new(state.cp1.fgr().read_word(fgr(raw.fs())));
    let class = source.classify(FloatNanMode::QuietBitSet);
    if unsupported_float_class(class) {
        return unimplemented(state);
    }
    let control = state
        .cp1
        .fcsr()
        .conversion_float_control(conversion_rounding(operation));
    if converts_to_long(operation) {
        let result = backend.f32_to_i64(control, source);
        if fixed_conversion_is_unimplemented(class, result.flags)
            || (!class.is_signaling_nan()
                && !(-(1_i64 << 53)..(1_i64 << 53)).contains(&result.value))
        {
            return unimplemented(state);
        }
        commit_i64(state, raw.fd(), result)
    } else {
        let result = backend.f32_to_i32(control, source);
        if fixed_conversion_is_unimplemented(class, result.flags) {
            return unimplemented(state);
        }
        commit_i32(state, raw.fd(), result)
    }
}

fn convert_double_to_integer<F: FloatBackend>(
    state: &mut Mips4ExecutionState,
    backend: &F,
    raw: Mips4Instruction,
    operation: Mips4Cp1Operation,
) -> Mips4FpuExecution {
    let mode = register_mode(state);
    let Some(bits) = read_doubleword(state, mode, raw.fs()) else {
        return unimplemented(state);
    };
    let source = Float64Bits::new(bits);
    let class = source.classify(FloatNanMode::QuietBitSet);
    if unsupported_float_class(class) {
        return unimplemented(state);
    }
    let control = state
        .cp1
        .fcsr()
        .conversion_float_control(conversion_rounding(operation));
    if converts_to_long(operation) {
        let result = backend.f64_to_i64(control, source);
        if fixed_conversion_is_unimplemented(class, result.flags)
            || (!class.is_signaling_nan()
                && !(-(1_i64 << 53)..(1_i64 << 53)).contains(&result.value))
        {
            return unimplemented(state);
        }
        commit_i64(state, raw.fd(), result)
    } else {
        let result = backend.f64_to_i32(control, source);
        if fixed_conversion_is_unimplemented(class, result.flags) {
            return unimplemented(state);
        }
        commit_i32(state, raw.fd(), result)
    }
}

fn fixed_conversion_is_unimplemented(class: FloatClass, flags: FloatExceptionFlags) -> bool {
    !class.is_signaling_nan() && flags.contains(FloatExceptionFlags::INVALID)
}

fn execute_compare<F: FloatBackend>(
    state: &mut Mips4ExecutionState,
    backend: &F,
    raw: Mips4Instruction,
    format: Mips4Cp1Format,
    condition: Mips4Cp1CompareCondition,
) -> Mips4FpuExecution {
    let compare_mode = if condition.function() & 0x08 != 0 {
        FloatCompareMode::Signaling
    } else {
        FloatCompareMode::Quiet
    };
    let control = state.cp1.fcsr().float_control();
    let result = match format {
        Mips4Cp1Format::Single => backend.compare_f32(
            control,
            compare_mode,
            Float32Bits::new(state.cp1.fgr().read_word(fgr(raw.fs()))),
            Float32Bits::new(state.cp1.fgr().read_word(fgr(raw.ft()))),
        ),
        Mips4Cp1Format::Double => {
            let mode = register_mode(state);
            let Some(fs) = read_doubleword(state, mode, raw.fs()) else {
                return unimplemented(state);
            };
            let Some(ft) = read_doubleword(state, mode, raw.ft()) else {
                return unimplemented(state);
            };
            backend.compare_f64(
                control,
                compare_mode,
                Float64Bits::new(fs),
                Float64Bits::new(ft),
            )
        }
        Mips4Cp1Format::Word | Mips4Cp1Format::Long => return unimplemented(state),
    };
    if let Err(exception) = state.cp1.fcsr_mut().record_float_flags(result.flags) {
        return Mips4FpuExecution::Exception(exception);
    }
    let predicate = condition.function() & 0x07;
    let value = match result.value {
        FloatRelation::Unordered => predicate & 0x01 != 0,
        FloatRelation::Equal => predicate & 0x02 != 0,
        FloatRelation::Less => predicate & 0x04 != 0,
        FloatRelation::Greater => false,
    };
    state
        .cp1
        .fcsr_mut()
        .set_condition_code(condition_code((raw.fd() >> 2) & 0x07), value);
    Mips4FpuExecution::Retire
}

fn prepare_offset_memory(
    state: &mut Mips4ExecutionState,
    policy: &impl Mips4ExecutionPolicy,
    raw: Mips4Instruction,
    operation: Mips4Cp1OffsetMemoryOperation,
    endianness: Mips4Endianness,
) -> Result<Mips4FpuExecution, Mips4MemoryAccessError> {
    let instruction = Mips4Cp1Instruction::from_instruction(raw).unwrap();
    let target = fgr(instruction.ft());
    let virtual_address =
        read_gpr(state, instruction.base()).wrapping_add(instruction.offset() as i64 as u64);
    let tlb_entries = state.deterministic_tlb_entries(policy, virtual_address);
    let access = Mips4Cp1OffsetMemoryAccess::prepare(
        operation,
        read_gpr(state, instruction.base()),
        instruction.offset(),
        target,
        endianness,
        policy.mmu_config(state.cp0.config()),
        state.cp0.status(),
        Mips4TlbAsid::new(state.cp0.entry_hi().address_space_identifier()),
        tlb_entries,
    )?;
    Ok(prepare_resolved_memory(
        state,
        policy,
        operation_is_load_offset(operation),
        target,
        virtual_address,
        access.access.physical_address(),
        access.access.cache_attribute(),
        operation_is_double_offset(operation),
        endianness,
    ))
}

fn prepare_indexed_memory(
    state: &mut Mips4ExecutionState,
    policy: &impl Mips4ExecutionPolicy,
    raw: Mips4Instruction,
    operation: Mips4Cp1IndexedMemoryOperation,
    endianness: Mips4Endianness,
) -> Result<Mips4FpuExecution, Mips4MemoryAccessError> {
    if raw.rd() != 0 {
        return Ok(Mips4FpuExecution::Retire);
    }
    let instruction = Mips4Cp1Instruction::from_instruction(raw).unwrap();
    let target = fgr(instruction.fd());
    let base = read_gpr(state, instruction.base());
    let index = read_gpr(state, instruction.index());
    let virtual_address = base.wrapping_add(index);
    let tlb_entries = state.deterministic_tlb_entries(policy, virtual_address);
    let access = Mips4Cp1IndexedMemoryAccess::prepare(
        operation,
        base,
        index,
        target,
        endianness,
        policy.mmu_config(state.cp0.config()),
        state.cp0.status(),
        Mips4TlbAsid::new(state.cp0.entry_hi().address_space_identifier()),
        tlb_entries,
    )?;
    Ok(prepare_resolved_memory(
        state,
        policy,
        operation_is_load_indexed(operation),
        target,
        virtual_address,
        access.access.physical_address(),
        access.access.cache_attribute(),
        operation_is_double_indexed(operation),
        endianness,
    ))
}

#[allow(clippy::too_many_arguments)]
fn prepare_resolved_memory(
    state: &mut Mips4ExecutionState,
    policy: &impl Mips4ExecutionPolicy,
    load: bool,
    target: Mips4Cp1FgrIndex,
    virtual_address: u64,
    physical_address: u64,
    cache_attribute: Mips4MmuCacheAttribute,
    doubleword: bool,
    endianness: Mips4Endianness,
) -> Mips4FpuExecution {
    let size = if doubleword {
        Mips4ExecutionTransferSize::Doubleword
    } else {
        Mips4ExecutionTransferSize::Word
    };
    let access_type = policy.resolve_access_type(cache_attribute);
    let cache_policy = policy.resolve_cache_policy(cache_attribute);
    if load {
        let mode = register_mode(state);
        if doubleword && !doubleword_register_valid(mode, target) {
            return unimplemented(state);
        }
        return Mips4FpuExecution::Read {
            pending: Mips4PendingFpuRead {
                target,
                size,
                register_mode: mode,
            },
            transaction: Mips4ExecutionTransaction::Read {
                physical_address,
                size,
                kind: Mips4ExecutionAccessKind::DataLoad,
                access_type,
            },
            virtual_address,
            cache_policy,
        };
    }
    let mode = register_mode(state);
    let value = if doubleword {
        let Some(value) = read_doubleword(state, mode, target.number()) else {
            return unimplemented(state);
        };
        value
    } else {
        u64::from(state.cp1.fgr().read_word(target))
    };
    Mips4FpuExecution::Write {
        transaction: Mips4ExecutionTransaction::Write {
            physical_address,
            size,
            data: encode_lanes(value, size, endianness),
            byte_enable: if doubleword { 0xff } else { 0x0f },
            access_type,
        },
        virtual_address,
        cache_policy,
    }
}

fn commit_f32(
    state: &mut Mips4ExecutionState,
    destination: u8,
    mut result: FloatResult<Float32Bits>,
) -> Mips4FpuExecution {
    let subnormal = matches!(
        result.value.classify(FloatNanMode::QuietBitSet),
        FloatClass::PositiveSubnormal | FloatClass::NegativeSubnormal
    );
    if underflow_is_unimplemented(state, result.flags, subnormal) {
        return unimplemented(state);
    }
    if (result.flags.contains(FloatExceptionFlags::UNDERFLOW) || subnormal)
        && state.cp1.fcsr().flush_to_zero()
    {
        result.flags |= FloatExceptionFlags::UNDERFLOW | FloatExceptionFlags::INEXACT;
        result.value =
            flush_underflow_f32(result.value, state.cp1.fcsr().float_control().rounding_mode);
    }
    if let Err(exception) = state.cp1.fcsr_mut().record_float_flags(result.flags) {
        return Mips4FpuExecution::Exception(exception);
    }
    state
        .cp1
        .fgr_mut()
        .write_word(fgr(destination), result.value.bits());
    Mips4FpuExecution::Retire
}

fn commit_f64(
    state: &mut Mips4ExecutionState,
    destination: u8,
    mut result: FloatResult<Float64Bits>,
) -> Mips4FpuExecution {
    let mode = register_mode(state);
    let target = fgr(destination);
    if !doubleword_register_valid(mode, target) {
        return unimplemented(state);
    }
    let subnormal = matches!(
        result.value.classify(FloatNanMode::QuietBitSet),
        FloatClass::PositiveSubnormal | FloatClass::NegativeSubnormal
    );
    if underflow_is_unimplemented(state, result.flags, subnormal) {
        return unimplemented(state);
    }
    if (result.flags.contains(FloatExceptionFlags::UNDERFLOW) || subnormal)
        && state.cp1.fcsr().flush_to_zero()
    {
        result.flags |= FloatExceptionFlags::UNDERFLOW | FloatExceptionFlags::INEXACT;
        result.value =
            flush_underflow_f64(result.value, state.cp1.fcsr().float_control().rounding_mode);
    }
    if let Err(exception) = state.cp1.fcsr_mut().record_float_flags(result.flags) {
        return Mips4FpuExecution::Exception(exception);
    }
    if state
        .cp1
        .fgr_mut()
        .write_doubleword(mode, target, result.value.bits())
        .is_err()
    {
        return unimplemented(state);
    }
    Mips4FpuExecution::Retire
}

fn commit_i32(
    state: &mut Mips4ExecutionState,
    destination: u8,
    result: FloatResult<i32>,
) -> Mips4FpuExecution {
    if let Err(exception) = state.cp1.fcsr_mut().record_float_flags(result.flags) {
        return Mips4FpuExecution::Exception(exception);
    }
    state
        .cp1
        .fgr_mut()
        .write_word(fgr(destination), result.value as u32);
    Mips4FpuExecution::Retire
}

fn commit_i64(
    state: &mut Mips4ExecutionState,
    destination: u8,
    result: FloatResult<i64>,
) -> Mips4FpuExecution {
    let mode = register_mode(state);
    let target = fgr(destination);
    if !doubleword_register_valid(mode, target) {
        return unimplemented(state);
    }
    if let Err(exception) = state.cp1.fcsr_mut().record_float_flags(result.flags) {
        return Mips4FpuExecution::Exception(exception);
    }
    if state
        .cp1
        .fgr_mut()
        .write_doubleword(mode, target, result.value as u64)
        .is_err()
    {
        return unimplemented(state);
    }
    Mips4FpuExecution::Retire
}

fn unary_f32(value: Float32Bits, negate: bool) -> FloatResult<Float32Bits> {
    let class = value.classify(FloatNanMode::QuietBitSet);
    let flags = if class.is_signaling_nan() {
        FloatExceptionFlags::INVALID
    } else {
        FloatExceptionFlags::empty()
    };
    let value = if class.is_signaling_nan() {
        Float32Bits::new(0x7fc0_0000)
    } else if negate {
        value.neg()
    } else {
        value.abs()
    };
    FloatResult::new(value, flags)
}

fn unary_f64(value: Float64Bits, negate: bool) -> FloatResult<Float64Bits> {
    let class = value.classify(FloatNanMode::QuietBitSet);
    let flags = if class.is_signaling_nan() {
        FloatExceptionFlags::INVALID
    } else {
        FloatExceptionFlags::empty()
    };
    let value = if class.is_signaling_nan() {
        Float64Bits::new(0x7ff8_0000_0000_0000)
    } else if negate {
        value.neg()
    } else {
        value.abs()
    };
    FloatResult::new(value, flags)
}

fn multiply_accumulate_f32<F: FloatBackend>(
    backend: &F,
    control: FloatControl,
    operation: Mips4Cp1Operation,
    fs: Float32Bits,
    ft: Float32Bits,
    fr: Float32Bits,
) -> Option<FloatResult<Float32Bits>> {
    let product = backend.mul_f32(control, fs, ft);
    if intermediate_is_unimplemented_f32(product) {
        return None;
    }
    let combined = if matches!(
        operation,
        Mips4Cp1Operation::MultiplyAdd | Mips4Cp1Operation::NegativeMultiplyAdd
    ) {
        backend.add_f32(control, product.value, fr)
    } else {
        backend.sub_f32(control, product.value, fr)
    };
    let value = if matches!(
        operation,
        Mips4Cp1Operation::NegativeMultiplyAdd | Mips4Cp1Operation::NegativeMultiplySubtract
    ) {
        combined.value.neg()
    } else {
        combined.value
    };
    Some(FloatResult::new(value, product.flags | combined.flags))
}

fn multiply_accumulate_f64<F: FloatBackend>(
    backend: &F,
    control: FloatControl,
    operation: Mips4Cp1Operation,
    fs: Float64Bits,
    ft: Float64Bits,
    fr: Float64Bits,
) -> Option<FloatResult<Float64Bits>> {
    let product = backend.mul_f64(control, fs, ft);
    if intermediate_is_unimplemented_f64(product) {
        return None;
    }
    let combined = if matches!(
        operation,
        Mips4Cp1Operation::MultiplyAdd | Mips4Cp1Operation::NegativeMultiplyAdd
    ) {
        backend.add_f64(control, product.value, fr)
    } else {
        backend.sub_f64(control, product.value, fr)
    };
    let value = if matches!(
        operation,
        Mips4Cp1Operation::NegativeMultiplyAdd | Mips4Cp1Operation::NegativeMultiplySubtract
    ) {
        combined.value.neg()
    } else {
        combined.value
    };
    Some(FloatResult::new(value, product.flags | combined.flags))
}

fn operation_uses_unsupported_inputs(
    operation: Mips4Cp1Operation,
    fs: FloatClass,
    ft: FloatClass,
    fr: FloatClass,
) -> bool {
    let fs_bad = unsupported_float_class(fs);
    let ft_bad = unsupported_float_class(ft);
    let fr_bad = unsupported_float_class(fr);
    match operation {
        Mips4Cp1Operation::Add
        | Mips4Cp1Operation::Subtract
        | Mips4Cp1Operation::Multiply
        | Mips4Cp1Operation::Divide => fs_bad || ft_bad,
        Mips4Cp1Operation::MultiplyAdd
        | Mips4Cp1Operation::MultiplySubtract
        | Mips4Cp1Operation::NegativeMultiplyAdd
        | Mips4Cp1Operation::NegativeMultiplySubtract => fs_bad || ft_bad || fr_bad,
        _ => fs_bad,
    }
}

fn unsupported_float_class(class: FloatClass) -> bool {
    matches!(
        class,
        FloatClass::PositiveSubnormal | FloatClass::NegativeSubnormal | FloatClass::QuietNan
    )
}

fn intermediate_is_unimplemented_f32(result: FloatResult<Float32Bits>) -> bool {
    result.flags.bits() & (FloatExceptionFlags::OVERFLOW | FloatExceptionFlags::UNDERFLOW).bits()
        != 0
        || matches!(
            result.value.classify(FloatNanMode::QuietBitSet),
            FloatClass::PositiveSubnormal | FloatClass::NegativeSubnormal
        )
}

fn intermediate_is_unimplemented_f64(result: FloatResult<Float64Bits>) -> bool {
    result.flags.bits() & (FloatExceptionFlags::OVERFLOW | FloatExceptionFlags::UNDERFLOW).bits()
        != 0
        || matches!(
            result.value.classify(FloatNanMode::QuietBitSet),
            FloatClass::PositiveSubnormal | FloatClass::NegativeSubnormal
        )
}

fn underflow_is_unimplemented(
    state: &Mips4ExecutionState,
    flags: FloatExceptionFlags,
    subnormal_result: bool,
) -> bool {
    if !flags.contains(FloatExceptionFlags::UNDERFLOW) && !subnormal_result {
        return false;
    }
    let fcsr = state.cp1.fcsr();
    let enabled = fcsr.enable_flags().bits()
        & (FloatExceptionFlags::UNDERFLOW | FloatExceptionFlags::INEXACT).bits()
        != 0;
    !fcsr.flush_to_zero() || enabled
}

fn flush_underflow_f32(value: Float32Bits, rounding: FloatRoundingMode) -> Float32Bits {
    match (rounding, value.sign_bit()) {
        (FloatRoundingMode::TowardPositive, false) => Float32Bits::new(0x0080_0000),
        (FloatRoundingMode::TowardNegative, true) => Float32Bits::new(0x8080_0000),
        (_, false) => Float32Bits::new(0),
        (_, true) => Float32Bits::new(0x8000_0000),
    }
}

fn flush_underflow_f64(value: Float64Bits, rounding: FloatRoundingMode) -> Float64Bits {
    match (rounding, value.sign_bit()) {
        (FloatRoundingMode::TowardPositive, false) => Float64Bits::new(0x0010_0000_0000_0000),
        (FloatRoundingMode::TowardNegative, true) => Float64Bits::new(0x8010_0000_0000_0000),
        (_, false) => Float64Bits::new(0),
        (_, true) => Float64Bits::new(0x8000_0000_0000_0000),
    }
}

fn conversion_rounding(operation: Mips4Cp1Operation) -> Mips4Cp1ConversionRoundingMode {
    match operation {
        Mips4Cp1Operation::RoundLong | Mips4Cp1Operation::RoundWord => {
            Mips4Cp1ConversionRoundingMode::Round
        }
        Mips4Cp1Operation::TruncLong | Mips4Cp1Operation::TruncWord => {
            Mips4Cp1ConversionRoundingMode::Trunc
        }
        Mips4Cp1Operation::CeilLong | Mips4Cp1Operation::CeilWord => {
            Mips4Cp1ConversionRoundingMode::Ceil
        }
        Mips4Cp1Operation::FloorLong | Mips4Cp1Operation::FloorWord => {
            Mips4Cp1ConversionRoundingMode::Floor
        }
        Mips4Cp1Operation::ConvertLong | Mips4Cp1Operation::ConvertWord => {
            Mips4Cp1ConversionRoundingMode::Fcsr
        }
        _ => unreachable!(),
    }
}

fn is_conversion_to_integer(operation: Mips4Cp1Operation) -> bool {
    matches!(
        operation,
        Mips4Cp1Operation::CeilLong
            | Mips4Cp1Operation::CeilWord
            | Mips4Cp1Operation::ConvertLong
            | Mips4Cp1Operation::ConvertWord
            | Mips4Cp1Operation::FloorLong
            | Mips4Cp1Operation::FloorWord
            | Mips4Cp1Operation::RoundLong
            | Mips4Cp1Operation::RoundWord
            | Mips4Cp1Operation::TruncLong
            | Mips4Cp1Operation::TruncWord
    )
}

fn converts_to_long(operation: Mips4Cp1Operation) -> bool {
    matches!(
        operation,
        Mips4Cp1Operation::CeilLong
            | Mips4Cp1Operation::ConvertLong
            | Mips4Cp1Operation::FloorLong
            | Mips4Cp1Operation::RoundLong
            | Mips4Cp1Operation::TruncLong
    )
}

fn is_multiply_accumulate(operation: Mips4Cp1Operation) -> bool {
    matches!(
        operation,
        Mips4Cp1Operation::MultiplyAdd
            | Mips4Cp1Operation::MultiplySubtract
            | Mips4Cp1Operation::NegativeMultiplyAdd
            | Mips4Cp1Operation::NegativeMultiplySubtract
    )
}

fn read_formatted(
    state: &Mips4ExecutionState,
    mode: Mips4Cp1RegisterMode,
    index: Mips4Cp1FgrIndex,
    format: Mips4Cp1Format,
) -> Option<u64> {
    match format {
        Mips4Cp1Format::Single | Mips4Cp1Format::Word => {
            Some(u64::from(state.cp1.fgr().read_word(index)))
        }
        Mips4Cp1Format::Double | Mips4Cp1Format::Long => {
            state.cp1.fgr().read_doubleword(mode, index).ok()
        }
    }
}

fn write_formatted(
    state: &mut Mips4ExecutionState,
    mode: Mips4Cp1RegisterMode,
    index: Mips4Cp1FgrIndex,
    format: Mips4Cp1Format,
    value: u64,
) -> Result<(), ()> {
    match format {
        Mips4Cp1Format::Single | Mips4Cp1Format::Word => {
            state.cp1.fgr_mut().write_word(index, value as u32);
            Ok(())
        }
        Mips4Cp1Format::Double | Mips4Cp1Format::Long => state
            .cp1
            .fgr_mut()
            .write_doubleword(mode, index, value)
            .map_err(|_| ()),
    }
}

fn register_mode(state: &Mips4ExecutionState) -> Mips4Cp1RegisterMode {
    if state.cp0.status().additional_float_registers() {
        Mips4Cp1RegisterMode::SixtyFourBit
    } else {
        Mips4Cp1RegisterMode::ThirtyTwoBit
    }
}

fn doubleword_register_valid(mode: Mips4Cp1RegisterMode, register: Mips4Cp1FgrIndex) -> bool {
    matches!(mode, Mips4Cp1RegisterMode::SixtyFourBit) || register.number() & 1 == 0
}

fn read_doubleword(
    state: &Mips4ExecutionState,
    mode: Mips4Cp1RegisterMode,
    register: u8,
) -> Option<u64> {
    state.cp1.fgr().read_doubleword(mode, fgr(register)).ok()
}

fn operation_is_load_offset(operation: Mips4Cp1OffsetMemoryOperation) -> bool {
    matches!(
        operation,
        Mips4Cp1OffsetMemoryOperation::LoadWord | Mips4Cp1OffsetMemoryOperation::LoadDoubleword
    )
}

fn operation_is_double_offset(operation: Mips4Cp1OffsetMemoryOperation) -> bool {
    matches!(
        operation,
        Mips4Cp1OffsetMemoryOperation::LoadDoubleword
            | Mips4Cp1OffsetMemoryOperation::StoreDoubleword
    )
}

fn operation_is_load_indexed(operation: Mips4Cp1IndexedMemoryOperation) -> bool {
    matches!(
        operation,
        Mips4Cp1IndexedMemoryOperation::LoadWordIndexed
            | Mips4Cp1IndexedMemoryOperation::LoadDoublewordIndexed
    )
}

fn operation_is_double_indexed(operation: Mips4Cp1IndexedMemoryOperation) -> bool {
    matches!(
        operation,
        Mips4Cp1IndexedMemoryOperation::LoadDoublewordIndexed
            | Mips4Cp1IndexedMemoryOperation::StoreDoublewordIndexed
    )
}

fn condition_code(value: u8) -> Mips4Cp1ConditionCode {
    Mips4Cp1ConditionCode::from_u8(value).unwrap()
}

fn fgr(value: u8) -> Mips4Cp1FgrIndex {
    Mips4Cp1FgrIndex::from_u8(value).unwrap()
}

fn read_gpr(state: &Mips4ExecutionState, register: u8) -> u64 {
    state.gpr.read(Mips4GprIndex::from_u8(register).unwrap())
}

fn write_gpr(state: &mut Mips4ExecutionState, register: u8, value: u64) {
    state
        .gpr
        .write(Mips4GprIndex::from_u8(register).unwrap(), value);
}

fn unimplemented(state: &mut Mips4ExecutionState) -> Mips4FpuExecution {
    Mips4FpuExecution::Exception(state.cp1.fcsr_mut().record_unimplemented_operation())
}
