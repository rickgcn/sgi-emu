use se_core::bus::PhysicalBus;
use se_float::{
    backend::Backend,
    format::{Float32, Float64},
    operation::{ComparisonMode, ExceptionFlags, Relation, RoundingMode},
};

use super::{
    ExecutionOutcome, InstructionResult,
    control::branch_resume_pc,
    cp0::Exception,
    decode::{
        Cp1BinaryOperation, Cp1Conversion, Cp1FloatFormat, Cp1Instruction, Cp1UnaryOperation,
    },
    load_store,
    state::{InstructionEffect, State},
};

const IMPLEMENTATION_REVISION: u32 = 0x0000_0300;
const CONTROL_STATUS_WRITABLE_MASK: u32 = 0x0083_ffff;
const CONTROL_STATUS_UNIMPLEMENTED: u32 = 1 << 17;
const CONTROL_STATUS_CAUSE_MASK: u32 = 0x0001_f000;
const CONTROL_STATUS_ENABLE_MASK: u32 = 0x0000_0f80;
const CONTROL_STATUS_CONDITION: u32 = 1 << 23;
const CONTROL_STATUS_FLAG_SHIFT: u32 = 2;
const CONTROL_STATUS_ENABLE_SHIFT: u32 = 7;
const CONTROL_STATUS_CAUSE_SHIFT: u32 = 12;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Fcr31Update {
    cause: ExceptionFlags,
    sticky_flags: ExceptionFlags,
    unimplemented: bool,
    writeback: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct Cp1 {
    registers: [u32; 32],
    exception_instruction: u32,
    control_status: u32,
    backend: Backend,
}

impl Cp1 {
    pub(super) const fn new(backend: Backend) -> Self {
        Self {
            registers: [0; 32],
            exception_instruction: 0,
            control_status: 0,
            backend,
        }
    }

    pub(super) fn read_general_register(&self, index: usize) -> u32 {
        self.registers[index]
    }

    pub(super) fn write_general_register(&mut self, index: usize, value: u32) {
        self.registers[index] = value;
    }

    pub(super) fn read_control_register(&self, index: usize) -> u32 {
        match index {
            0 => IMPLEMENTATION_REVISION,
            30 => self.exception_instruction,
            31 => self.control_status,
            _ => 0,
        }
    }

    pub(super) fn write_control_register(&mut self, index: usize, value: u32) {
        match index {
            30 => self.exception_instruction = value,
            31 => self.write_control_status(value),
            _ => {}
        }
    }

    pub(super) fn condition(&self) -> bool {
        self.control_status & CONTROL_STATUS_CONDITION != 0
    }

    pub(super) fn interrupt_asserted(&self) -> bool {
        let enabled_causes = (self.control_status & CONTROL_STATUS_CAUSE_MASK) >> 5
            & self.control_status
            & CONTROL_STATUS_ENABLE_MASK;

        self.control_status & CONTROL_STATUS_UNIMPLEMENTED != 0 || enabled_causes != 0
    }

    pub(super) fn write_condition(&mut self, value: bool) {
        if value {
            self.control_status |= CONTROL_STATUS_CONDITION;
        } else {
            self.control_status &= !CONTROL_STATUS_CONDITION;
        }
    }

    fn read_float32(&self, register: usize) -> Float32 {
        Float32::from_bits(self.registers[register])
    }

    fn read_float64(&self, register: usize) -> Float64 {
        let even = register & !1;
        Float64::from_bits(
            u64::from(self.registers[even]) | u64::from(self.registers[even | 1]) << 32,
        )
    }

    fn read_word(&self, register: usize) -> u32 {
        self.registers[register]
    }

    fn write_float32(&mut self, register: usize, value: Float32) {
        self.registers[register] = value.to_bits();
    }

    fn write_float64(&mut self, register: usize, value: Float64) {
        let even = register & !1;
        let bits = value.to_bits();
        self.registers[even] = bits as u32;
        self.registers[even | 1] = (bits >> 32) as u32;
    }

    fn write_word(&mut self, register: usize, value: u32) {
        self.registers[register] = value;
    }

    fn rounding_mode(&self) -> RoundingMode {
        match self.control_status & 0x3 {
            0 => RoundingMode::NearestEven,
            1 => RoundingMode::TowardZero,
            2 => RoundingMode::TowardPositive,
            3 => RoundingMode::TowardNegative,
            _ => unreachable!("the rounding mode occupies two bits"),
        }
    }

    fn enabled_exceptions(&self) -> ExceptionFlags {
        ExceptionFlags::from_bits_retain(
            ((self.control_status & CONTROL_STATUS_ENABLE_MASK) >> CONTROL_STATUS_ENABLE_SHIFT)
                as u8,
        )
    }

    fn classify_float32_operands(
        &self,
        operands: &[Float32],
        signal_any_nan: bool,
    ) -> Option<Fcr31Update> {
        if operands.iter().copied().any(Float32::is_subnormal) {
            return Some(Self::unimplemented_update());
        }

        if operands.iter().copied().any(Float32::is_nan) {
            let signals_invalid =
                signal_any_nan || operands.iter().copied().any(Float32::is_signaling_nan);
            return Some(self.classify_nan(signals_invalid));
        }

        None
    }

    fn classify_float64_operands(
        &self,
        operands: &[Float64],
        signal_any_nan: bool,
    ) -> Option<Fcr31Update> {
        if operands.iter().copied().any(Float64::is_subnormal) {
            return Some(Self::unimplemented_update());
        }

        if operands.iter().copied().any(Float64::is_nan) {
            let signals_invalid =
                signal_any_nan || operands.iter().copied().any(Float64::is_signaling_nan);
            return Some(self.classify_nan(signals_invalid));
        }

        None
    }

    fn classify_nan(&self, signals_invalid: bool) -> Fcr31Update {
        if signals_invalid {
            self.classify_invalid()
        } else {
            Self::unimplemented_update()
        }
    }

    fn classify_invalid(&self) -> Fcr31Update {
        if self.enabled_exceptions().contains(ExceptionFlags::INVALID) {
            Fcr31Update {
                cause: ExceptionFlags::INVALID,
                sticky_flags: ExceptionFlags::empty(),
                unimplemented: false,
                writeback: false,
            }
        } else {
            Self::unimplemented_update()
        }
    }

    fn classify_outcome(&self, flags: ExceptionFlags, tiny: bool) -> Fcr31Update {
        if flags.contains(ExceptionFlags::INVALID) {
            return self.classify_invalid();
        }

        if flags.contains(ExceptionFlags::DIVIDE_BY_ZERO)
            || tiny
            || flags.contains(ExceptionFlags::UNDERFLOW)
        {
            return Self::unimplemented_update();
        }

        let supported_flags = flags & (ExceptionFlags::OVERFLOW | ExceptionFlags::INEXACT);
        let enabled = supported_flags & self.enabled_exceptions();
        Fcr31Update {
            cause: supported_flags,
            sticky_flags: if enabled.is_empty() {
                supported_flags
            } else {
                ExceptionFlags::empty()
            },
            unimplemented: false,
            writeback: enabled.is_empty(),
        }
    }

    fn apply_fcr31_update(&mut self, update: Fcr31Update) {
        self.control_status &= !(CONTROL_STATUS_CAUSE_MASK | CONTROL_STATUS_UNIMPLEMENTED);
        self.control_status |= u32::from(update.cause.bits()) << CONTROL_STATUS_CAUSE_SHIFT;
        self.control_status |= u32::from(update.sticky_flags.bits()) << CONTROL_STATUS_FLAG_SHIFT;
        if update.unimplemented {
            self.control_status |= CONTROL_STATUS_UNIMPLEMENTED;
        }
    }

    const fn unimplemented_update() -> Fcr31Update {
        Fcr31Update {
            cause: ExceptionFlags::empty(),
            sticky_flags: ExceptionFlags::empty(),
            unimplemented: true,
            writeback: false,
        }
    }

    fn write_control_status(&mut self, value: u32) {
        self.control_status = value & CONTROL_STATUS_WRITABLE_MASK;
    }
}

pub(super) fn execute(
    state: &mut State,
    instruction: Cp1Instruction,
    bus: &mut dyn PhysicalBus,
) -> InstructionResult {
    if !state.coprocessor_usable(1) {
        return Ok(ExecutionOutcome::Exception(
            Exception::CoprocessorUnusable { unit: 1 },
        ));
    }

    let completion = match instruction {
        Cp1Instruction::Mfc1 { rt, rd } => (
            None,
            Some(InstructionEffect::DelayedGprWrite {
                index: rt,
                value: state.read_cp1_general(rd),
                load_merge_bypass: false,
            }),
        ),
        Cp1Instruction::Cfc1 { rt, rd } => (
            None,
            Some(InstructionEffect::DelayedGprWrite {
                index: rt,
                value: state.read_cp1_control(rd),
                load_merge_bypass: false,
            }),
        ),
        Cp1Instruction::Mtc1 { rt, rd } => (
            None,
            Some(InstructionEffect::DelayedCp1GeneralWrite {
                index: rd,
                value: state.read_gpr(rt),
            }),
        ),
        Cp1Instruction::Ctc1 { rt, rd } => (
            None,
            Some(InstructionEffect::DelayedCp1ControlWrite {
                index: rd,
                value: state.read_gpr(rt),
            }),
        ),
        Cp1Instruction::Bc1f { offset } => (
            Some(branch_resume_pc(state.pc(), offset, !state.cp1_condition())),
            None,
        ),
        Cp1Instruction::Bc1t { offset } => (
            Some(branch_resume_pc(state.pc(), offset, state.cp1_condition())),
            None,
        ),
        Cp1Instruction::Lwc1 { base, ft, offset } => {
            let mut bytes = [0; 4];
            match load_store::load(state, base, offset, &mut bytes, bus)? {
                ExecutionOutcome::Completed(()) => {}
                ExecutionOutcome::Exception(exception) => {
                    return Ok(ExecutionOutcome::Exception(exception));
                }
            }

            (
                None,
                Some(InstructionEffect::DelayedCp1GeneralWrite {
                    index: ft,
                    value: u32::from_be_bytes(bytes),
                }),
            )
        }
        Cp1Instruction::Swc1 { base, ft, offset } => {
            let bytes = state.read_cp1_general(ft).to_be_bytes();
            match load_store::store(state, base, offset, &bytes, bus)? {
                ExecutionOutcome::Completed(()) => {}
                ExecutionOutcome::Exception(exception) => {
                    return Ok(ExecutionOutcome::Exception(exception));
                }
            }

            (None, None)
        }
        Cp1Instruction::Binary {
            operation,
            format,
            ft,
            fs,
            fd,
        } => {
            execute_binary(state, operation, format, ft, fs, fd);
            (None, None)
        }
        Cp1Instruction::Unary {
            operation,
            format,
            fs,
            fd,
        } => {
            execute_unary(state, operation, format, fs, fd);
            (None, None)
        }
        Cp1Instruction::Convert { operation, fs, fd } => {
            execute_conversion(state, operation, fs, fd);
            (None, None)
        }
        Cp1Instruction::Compare {
            format,
            condition,
            fs,
            ft,
        } => (None, execute_compare(state, format, condition, fs, ft)),
        Cp1Instruction::UnimplementedOperation => {
            let update = Cp1::unimplemented_update();
            state.commit_pending_cp1_write();
            state.cp1_mut().apply_fcr31_update(update);
            (None, None)
        }
    };

    Ok(ExecutionOutcome::Completed(completion))
}

fn execute_binary(
    state: &mut State,
    operation: Cp1BinaryOperation,
    format: Cp1FloatFormat,
    ft: usize,
    fs: usize,
    fd: usize,
) {
    match format {
        Cp1FloatFormat::Single => {
            let (update, value) = {
                let cp1 = state.cp1();
                let lhs = cp1.read_float32(fs);
                let rhs = cp1.read_float32(ft);
                let operands = [lhs, rhs];
                match cp1.classify_float32_operands(&operands, false) {
                    Some(update) => (update, None),
                    None => {
                        let outcome = match operation {
                            Cp1BinaryOperation::Add => {
                                cp1.backend.add_f32(lhs, rhs, cp1.rounding_mode())
                            }
                            Cp1BinaryOperation::Subtract => {
                                cp1.backend.sub_f32(lhs, rhs, cp1.rounding_mode())
                            }
                            Cp1BinaryOperation::Multiply => {
                                cp1.backend.mul_f32(lhs, rhs, cp1.rounding_mode())
                            }
                            Cp1BinaryOperation::Divide => {
                                cp1.backend.div_f32(lhs, rhs, cp1.rounding_mode())
                            }
                        };
                        (
                            cp1.classify_outcome(outcome.flags, outcome.tiny),
                            Some(outcome.value),
                        )
                    }
                }
            };
            finish_float32(state, fd, update, value);
        }
        Cp1FloatFormat::Double => {
            let (update, value) = {
                let cp1 = state.cp1();
                let lhs = cp1.read_float64(fs);
                let rhs = cp1.read_float64(ft);
                let operands = [lhs, rhs];
                match cp1.classify_float64_operands(&operands, false) {
                    Some(update) => (update, None),
                    None => {
                        let outcome = match operation {
                            Cp1BinaryOperation::Add => {
                                cp1.backend.add_f64(lhs, rhs, cp1.rounding_mode())
                            }
                            Cp1BinaryOperation::Subtract => {
                                cp1.backend.sub_f64(lhs, rhs, cp1.rounding_mode())
                            }
                            Cp1BinaryOperation::Multiply => {
                                cp1.backend.mul_f64(lhs, rhs, cp1.rounding_mode())
                            }
                            Cp1BinaryOperation::Divide => {
                                cp1.backend.div_f64(lhs, rhs, cp1.rounding_mode())
                            }
                        };
                        (
                            cp1.classify_outcome(outcome.flags, outcome.tiny),
                            Some(outcome.value),
                        )
                    }
                }
            };
            finish_float64(state, fd, update, value);
        }
    }
}

fn execute_unary(
    state: &mut State,
    operation: Cp1UnaryOperation,
    format: Cp1FloatFormat,
    fs: usize,
    fd: usize,
) {
    if operation == Cp1UnaryOperation::Move {
        match format {
            Cp1FloatFormat::Single => {
                let value = state.cp1().read_float32(fs);
                state.commit_pending_cp1_write();
                state.cp1_mut().write_float32(fd, value);
            }
            Cp1FloatFormat::Double => {
                let value = state.cp1().read_float64(fs);
                state.commit_pending_cp1_write();
                state.cp1_mut().write_float64(fd, value);
            }
        }
        return;
    }

    match format {
        Cp1FloatFormat::Single => {
            let (update, value) = {
                let cp1 = state.cp1();
                let source = cp1.read_float32(fs);
                match cp1.classify_float32_operands(&[source], false) {
                    Some(update) => (update, None),
                    None => {
                        let outcome = match operation {
                            Cp1UnaryOperation::Absolute => cp1.backend.abs_f32(source),
                            Cp1UnaryOperation::Negate => cp1.backend.neg_f32(source),
                            Cp1UnaryOperation::Move => {
                                unreachable!("move returns before arithmetic execution")
                            }
                        };
                        (
                            cp1.classify_outcome(outcome.flags, outcome.tiny),
                            Some(outcome.value),
                        )
                    }
                }
            };
            finish_float32(state, fd, update, value);
        }
        Cp1FloatFormat::Double => {
            let (update, value) = {
                let cp1 = state.cp1();
                let source = cp1.read_float64(fs);
                match cp1.classify_float64_operands(&[source], false) {
                    Some(update) => (update, None),
                    None => {
                        let outcome = match operation {
                            Cp1UnaryOperation::Absolute => cp1.backend.abs_f64(source),
                            Cp1UnaryOperation::Negate => cp1.backend.neg_f64(source),
                            Cp1UnaryOperation::Move => {
                                unreachable!("move returns before arithmetic execution")
                            }
                        };
                        (
                            cp1.classify_outcome(outcome.flags, outcome.tiny),
                            Some(outcome.value),
                        )
                    }
                }
            };
            finish_float64(state, fd, update, value);
        }
    }
}

fn execute_conversion(state: &mut State, operation: Cp1Conversion, fs: usize, fd: usize) {
    match operation {
        Cp1Conversion::SingleToDouble => {
            let (update, value) = {
                let cp1 = state.cp1();
                let source = cp1.read_float32(fs);
                match cp1.classify_float32_operands(&[source], false) {
                    Some(update) => (update, None),
                    None => {
                        let outcome = cp1.backend.convert_float32_to_float64(source);
                        (
                            cp1.classify_outcome(outcome.flags, outcome.tiny),
                            Some(outcome.value),
                        )
                    }
                }
            };
            finish_float64(state, fd, update, value);
        }
        Cp1Conversion::WordToDouble => {
            let (update, value) = {
                let cp1 = state.cp1();
                let outcome = cp1.backend.convert_i32_to_float64(cp1.read_word(fs) as i32);
                (
                    cp1.classify_outcome(outcome.flags, outcome.tiny),
                    Some(outcome.value),
                )
            };
            finish_float64(state, fd, update, value);
        }
        Cp1Conversion::DoubleToSingle => {
            let (update, value) = {
                let cp1 = state.cp1();
                let source = cp1.read_float64(fs);
                match cp1.classify_float64_operands(&[source], false) {
                    Some(update) => (update, None),
                    None => {
                        let outcome = cp1
                            .backend
                            .convert_float64_to_float32(source, cp1.rounding_mode());
                        (
                            cp1.classify_outcome(outcome.flags, outcome.tiny),
                            Some(outcome.value),
                        )
                    }
                }
            };
            finish_float32(state, fd, update, value);
        }
        Cp1Conversion::WordToSingle => {
            let (update, value) = {
                let cp1 = state.cp1();
                let outcome = cp1
                    .backend
                    .convert_i32_to_float32(cp1.read_word(fs) as i32, cp1.rounding_mode());
                (
                    cp1.classify_outcome(outcome.flags, outcome.tiny),
                    Some(outcome.value),
                )
            };
            finish_float32(state, fd, update, value);
        }
        Cp1Conversion::SingleToWord => {
            let (update, value) = {
                let cp1 = state.cp1();
                let source = cp1.read_float32(fs);
                match cp1.classify_float32_operands(&[source], false) {
                    Some(update) => (update, None),
                    None => {
                        let outcome = cp1
                            .backend
                            .convert_float32_to_i32(source, cp1.rounding_mode());
                        (
                            cp1.classify_outcome(outcome.flags, outcome.tiny),
                            Some(outcome.value as u32),
                        )
                    }
                }
            };
            finish_word(state, fd, update, value);
        }
        Cp1Conversion::DoubleToWord => {
            let (update, value) = {
                let cp1 = state.cp1();
                let source = cp1.read_float64(fs);
                match cp1.classify_float64_operands(&[source], false) {
                    Some(update) => (update, None),
                    None => {
                        let outcome = cp1
                            .backend
                            .convert_float64_to_i32(source, cp1.rounding_mode());
                        (
                            cp1.classify_outcome(outcome.flags, outcome.tiny),
                            Some(outcome.value as u32),
                        )
                    }
                }
            };
            finish_word(state, fd, update, value);
        }
    }
}

fn execute_compare(
    state: &mut State,
    format: Cp1FloatFormat,
    condition: u8,
    fs: usize,
    ft: usize,
) -> Option<InstructionEffect> {
    let mode = if condition & 0x8 == 0 {
        ComparisonMode::Quiet
    } else {
        ComparisonMode::Signaling
    };
    let signal_any_nan = mode == ComparisonMode::Signaling;

    let (update, relation) = match format {
        Cp1FloatFormat::Single => {
            let cp1 = state.cp1();
            let lhs = cp1.read_float32(fs);
            let rhs = cp1.read_float32(ft);
            match cp1.classify_float32_operands(&[lhs, rhs], signal_any_nan) {
                Some(update) => (update, None),
                None => {
                    let outcome = cp1.backend.compare_f32(lhs, rhs, mode);
                    (
                        cp1.classify_outcome(outcome.flags, outcome.tiny),
                        Some(outcome.value),
                    )
                }
            }
        }
        Cp1FloatFormat::Double => {
            let cp1 = state.cp1();
            let lhs = cp1.read_float64(fs);
            let rhs = cp1.read_float64(ft);
            match cp1.classify_float64_operands(&[lhs, rhs], signal_any_nan) {
                Some(update) => (update, None),
                None => {
                    let outcome = cp1.backend.compare_f64(lhs, rhs, mode);
                    (
                        cp1.classify_outcome(outcome.flags, outcome.tiny),
                        Some(outcome.value),
                    )
                }
            }
        }
    };

    state.commit_pending_cp1_write();
    state.cp1_mut().apply_fcr31_update(update);
    if !update.writeback {
        return None;
    }

    let relation = relation.expect("successful comparison must produce a relation");
    Some(InstructionEffect::DelayedCp1ConditionWrite {
        value: comparison_condition(condition, relation),
    })
}

fn finish_float32(
    state: &mut State,
    destination: usize,
    update: Fcr31Update,
    value: Option<Float32>,
) {
    state.commit_pending_cp1_write();
    let cp1 = state.cp1_mut();
    cp1.apply_fcr31_update(update);
    if update.writeback {
        cp1.write_float32(
            destination,
            value.expect("enabled floating-point writeback must have a result"),
        );
    }
}

fn finish_float64(
    state: &mut State,
    destination: usize,
    update: Fcr31Update,
    value: Option<Float64>,
) {
    state.commit_pending_cp1_write();
    let cp1 = state.cp1_mut();
    cp1.apply_fcr31_update(update);
    if update.writeback {
        cp1.write_float64(
            destination,
            value.expect("enabled floating-point writeback must have a result"),
        );
    }
}

fn finish_word(state: &mut State, destination: usize, update: Fcr31Update, value: Option<u32>) {
    state.commit_pending_cp1_write();
    let cp1 = state.cp1_mut();
    cp1.apply_fcr31_update(update);
    if update.writeback {
        cp1.write_word(
            destination,
            value.expect("enabled floating-point writeback must have a result"),
        );
    }
}

fn comparison_condition(condition: u8, relation: Relation) -> bool {
    match relation {
        Relation::Unordered => condition & 0x1 != 0,
        Relation::Equal => condition & 0x2 != 0,
        Relation::Less => condition & 0x4 != 0,
        Relation::Greater => false,
    }
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusFault, PhysAddr, PhysicalBus};
    use se_float::{
        backend::Backend,
        format::{Float32, Float64},
        operation::ExceptionFlags,
    };

    use super::{
        CONTROL_STATUS_CAUSE_MASK, CONTROL_STATUS_CONDITION, CONTROL_STATUS_ENABLE_MASK,
        CONTROL_STATUS_UNIMPLEMENTED, CONTROL_STATUS_WRITABLE_MASK, Cp1, Cp1BinaryOperation,
        Cp1Conversion, Cp1FloatFormat, Cp1Instruction, Cp1UnaryOperation, Exception,
        ExecutionOutcome, IMPLEMENTATION_REVISION, InstructionEffect, RoundingMode, State, execute,
    };
    use crate::mips1::r3000::{R3000Config, TEST_CONFIG};

    const STATUS_CU1: u32 = 1 << 29;
    const FCR31_FLAG_SHIFT: u32 = 2;
    const FCR31_ENABLE_SHIFT: u32 = 7;
    const FCR31_CAUSE_SHIFT: u32 = 12;
    const FCR31_FLAG_MASK: u32 = 0x0000_007c;

    struct TestBus {
        read_data: [u8; 4],
        reads: Vec<(PhysAddr, usize)>,
        writes: Vec<(PhysAddr, Vec<u8>)>,
    }

    impl TestBus {
        fn new(read_data: [u8; 4]) -> Self {
            Self {
                read_data,
                reads: Vec::new(),
                writes: Vec::new(),
            }
        }
    }

    impl PhysicalBus for TestBus {
        fn read(&mut self, address: PhysAddr, data: &mut [u8]) -> Result<(), BusFault> {
            self.reads.push((address, data.len()));
            data.copy_from_slice(&self.read_data[..data.len()]);
            Ok(())
        }

        fn write(&mut self, address: PhysAddr, data: &[u8]) -> Result<(), BusFault> {
            self.writes.push((address, data.to_vec()));
            Ok(())
        }
    }

    fn enabled_state() -> State {
        let mut state = State::new(TEST_CONFIG);
        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp0Write {
                index: 12,
                value: state.read_cp0(12) | STATUS_CU1,
            }),
        );
        state.complete_instruction(None, None);
        state.complete_instruction(None, None);
        state
    }

    fn run(state: &mut State, instruction: Cp1Instruction) {
        let mut bus = TestBus::new([0; 4]);
        let result = execute(state, instruction, &mut bus).expect("CP1 execution should succeed");
        let ExecutionOutcome::Completed((resume_pc, effect)) = result else {
            panic!("CP1 computation should not enter a CPU exception");
        };
        state.complete_instruction(resume_pc, effect);
    }

    fn binary(
        operation: Cp1BinaryOperation,
        format: Cp1FloatFormat,
        ft: usize,
        fs: usize,
        fd: usize,
    ) -> Cp1Instruction {
        Cp1Instruction::Binary {
            operation,
            format,
            ft,
            fs,
            fd,
        }
    }

    fn exception_bits(flags: ExceptionFlags, shift: u32) -> u32 {
        u32::from(flags.bits()) << shift
    }

    fn assert_unimplemented_result(
        state: &mut State,
        instruction: Cp1Instruction,
        initial_status: u32,
        destination: usize,
    ) {
        const EIR: u32 = 0x1234_5678;
        const DESTINATION: u32 = 0x89ab_cdef;

        state.cp1_mut().write_control_register(30, EIR);
        state.cp1_mut().write_control_register(31, initial_status);
        state
            .cp1_mut()
            .write_general_register(destination, DESTINATION);
        run(state, instruction);

        let status = state.read_cp1_control(31);
        assert_ne!(status & CONTROL_STATUS_UNIMPLEMENTED, 0);
        assert_eq!(status & CONTROL_STATUS_CAUSE_MASK, 0);
        assert_eq!(status & FCR31_FLAG_MASK, initial_status & FCR31_FLAG_MASK);
        assert_eq!(state.read_cp1_control(30), EIR);
        assert_eq!(state.read_cp1_general(destination), DESTINATION);
        assert!(state.cp1_interrupt_asserted());
    }

    #[test]
    fn new_initializes_deterministic_state() {
        let cp1 = Cp1::new(Backend::SoftFloat);

        for index in 0..32 {
            assert_eq!(cp1.read_general_register(index), 0);
        }
        assert_eq!(cp1.read_control_register(0), IMPLEMENTATION_REVISION);
        assert_eq!(cp1.read_control_register(30), 0);
        assert_eq!(cp1.read_control_register(31), 0);
        assert!(!cp1.condition());
        assert!(!cp1.interrupt_asserted());
    }

    #[test]
    fn general_registers_include_zero_and_thirty_one() {
        let mut cp1 = Cp1::new(Backend::SoftFloat);

        cp1.write_general_register(0, 0x1234_5678);
        cp1.write_general_register(31, 0x89ab_cdef);

        assert_eq!(cp1.read_general_register(0), 0x1234_5678);
        assert_eq!(cp1.read_general_register(31), 0x89ab_cdef);
    }

    #[test]
    fn control_registers_apply_their_access_rules() {
        let mut cp1 = Cp1::new(Backend::SoftFloat);

        cp1.write_control_register(0, u32::MAX);
        cp1.write_control_register(1, u32::MAX);
        cp1.write_control_register(29, u32::MAX);
        cp1.write_control_register(30, 0x1234_5678);
        cp1.write_control_register(31, u32::MAX);

        assert_eq!(cp1.read_control_register(0), IMPLEMENTATION_REVISION);
        for index in 1..30 {
            assert_eq!(cp1.read_control_register(index), 0);
        }
        assert_eq!(cp1.read_control_register(30), 0x1234_5678);
        assert_eq!(cp1.read_control_register(31), CONTROL_STATUS_WRITABLE_MASK);
    }

    #[test]
    fn condition_and_interrupt_are_derived_from_control_status() {
        let mut cp1 = Cp1::new(Backend::SoftFloat);

        cp1.write_control_register(31, CONTROL_STATUS_CONDITION);
        assert!(cp1.condition());
        assert!(!cp1.interrupt_asserted());

        cp1.write_control_register(31, CONTROL_STATUS_UNIMPLEMENTED);
        assert!(!cp1.condition());
        assert!(cp1.interrupt_asserted());

        for cause in 0..5 {
            let cause_bit = 1 << (12 + cause);
            let enable_bit = 1 << (7 + cause);

            cp1.write_control_register(31, cause_bit);
            assert!(!cp1.interrupt_asserted());

            cp1.write_control_register(31, enable_bit);
            assert!(!cp1.interrupt_asserted());

            cp1.write_control_register(31, cause_bit | enable_bit);
            assert!(cp1.interrupt_asserted());
        }

        cp1.write_control_register(
            31,
            (CONTROL_STATUS_CAUSE_MASK & !(1 << 12)) | (CONTROL_STATUS_ENABLE_MASK & (1 << 7)),
        );
        assert!(!cp1.interrupt_asserted());
    }

    #[test]
    fn conditional_branches_read_the_committed_condition() {
        let mut state = enabled_state();
        let mut bus = TestBus::new([0; 4]);
        let pc = state.pc();

        assert_eq!(
            execute(&mut state, Cp1Instruction::Bc1f { offset: 2 }, &mut bus,),
            Ok(ExecutionOutcome::Completed((
                Some(pc.wrapping_add(12)),
                None
            )))
        );
        assert_eq!(
            execute(&mut state, Cp1Instruction::Bc1t { offset: 2 }, &mut bus,),
            Ok(ExecutionOutcome::Completed((
                Some(pc.wrapping_add(8)),
                None
            )))
        );

        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp1ControlWrite {
                index: 31,
                value: CONTROL_STATUS_CONDITION,
            }),
        );
        state.complete_instruction(None, None);
        let pc = state.pc();

        assert_eq!(
            execute(&mut state, Cp1Instruction::Bc1f { offset: 2 }, &mut bus,),
            Ok(ExecutionOutcome::Completed((
                Some(pc.wrapping_add(8)),
                None
            )))
        );
        assert_eq!(
            execute(&mut state, Cp1Instruction::Bc1t { offset: 2 }, &mut bus,),
            Ok(ExecutionOutcome::Completed((
                Some(pc.wrapping_add(12)),
                None
            )))
        );
    }

    #[test]
    fn word_memory_uses_shared_big_endian_access_and_load_delay() {
        let mut state = enabled_state();
        state.write_gpr(1, 0xa000_0100);
        let mut bus = TestBus::new([0x12, 0x34, 0x56, 0x78]);
        let effect = InstructionEffect::DelayedCp1GeneralWrite {
            index: 5,
            value: 0x1234_5678,
        };

        assert_eq!(
            execute(
                &mut state,
                Cp1Instruction::Lwc1 {
                    base: 1,
                    ft: 5,
                    offset: 0,
                },
                &mut bus,
            ),
            Ok(ExecutionOutcome::Completed((None, Some(effect))))
        );
        assert_eq!(bus.reads, [(PhysAddr::new(0x100), 4)]);
        assert_eq!(state.read_cp1_general(5), 0);

        state.complete_instruction(None, Some(effect));
        assert_eq!(state.read_cp1_general(5), 0);
        state.complete_instruction(None, None);
        assert_eq!(state.read_cp1_general(5), 0x1234_5678);

        assert_eq!(
            execute(
                &mut state,
                Cp1Instruction::Swc1 {
                    base: 1,
                    ft: 5,
                    offset: 4,
                },
                &mut bus,
            ),
            Ok(ExecutionOutcome::Completed((None, None)))
        );
        assert_eq!(
            bus.writes,
            [(PhysAddr::new(0x104), vec![0x12, 0x34, 0x56, 0x78])]
        );
    }

    #[test]
    fn word_memory_reuses_word_alignment_exceptions() {
        let mut state = enabled_state();
        state.write_gpr(1, 0xa000_0101);
        let mut bus = TestBus::new([0; 4]);

        assert_eq!(
            execute(
                &mut state,
                Cp1Instruction::Lwc1 {
                    base: 1,
                    ft: 2,
                    offset: 0,
                },
                &mut bus,
            ),
            Ok(ExecutionOutcome::Exception(Exception::LoadAddressError {
                address: 0xa000_0101,
            }))
        );
        assert_eq!(
            execute(
                &mut state,
                Cp1Instruction::Swc1 {
                    base: 1,
                    ft: 2,
                    offset: 0,
                },
                &mut bus,
            ),
            Ok(ExecutionOutcome::Exception(Exception::StoreAddressError {
                address: 0xa000_0101,
            }))
        );
        assert!(bus.reads.is_empty());
        assert!(bus.writes.is_empty());
    }

    #[test]
    fn usability_gate_precedes_cp1_execution() {
        let mut state = State::new(TEST_CONFIG);
        state.write_gpr(1, 0xa000_0101);
        let mut bus = TestBus::new([0; 4]);

        assert_eq!(
            execute(
                &mut state,
                Cp1Instruction::Lwc1 {
                    base: 1,
                    ft: 2,
                    offset: 0,
                },
                &mut bus,
            ),
            Ok(ExecutionOutcome::Exception(
                Exception::CoprocessorUnusable { unit: 1 }
            ))
        );
        assert!(bus.reads.is_empty());
        assert!(bus.writes.is_empty());
    }

    #[test]
    fn configuration_selects_and_reset_preserves_the_backend() {
        let config = R3000Config::new(4 * 1024, 4 * 1024, 4, 4, true, Backend::Native);
        let mut state = State::new(config);
        state.cp1_mut().write_general_register(5, 0x1234_5678);
        state.cp1_mut().write_control_register(30, 0x89ab_cdef);
        state.cp1_mut().write_control_register(31, 0x0002_0000);
        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp1ConditionWrite { value: true }),
        );

        state.reset();

        assert_eq!(state.cp1().backend, Backend::Native);
        assert_eq!(state.read_cp1_general(5), 0x1234_5678);
        assert_eq!(state.read_cp1_control(30), 0x89ab_cdef);
        assert_eq!(state.read_cp1_control(31), 0x0002_0000);
        state.complete_instruction(None, None);
        assert!(!state.cp1_condition());
    }

    #[test]
    fn binary_arithmetic_executes_both_formats() {
        let cases = [
            (Cp1BinaryOperation::Add, 7.0_f32, 2.0_f32, 9.0_f32),
            (Cp1BinaryOperation::Subtract, 7.0, 2.0, 5.0),
            (Cp1BinaryOperation::Multiply, 7.0, 2.0, 14.0),
            (Cp1BinaryOperation::Divide, 7.0, 2.0, 3.5),
        ];

        for (operation, lhs, rhs, expected) in cases {
            let mut state = enabled_state();
            state
                .cp1_mut()
                .write_float32(1, Float32::from_bits(lhs.to_bits()));
            state
                .cp1_mut()
                .write_float32(2, Float32::from_bits(rhs.to_bits()));

            run(
                &mut state,
                binary(operation, Cp1FloatFormat::Single, 2, 1, 3),
            );

            assert_eq!(state.read_cp1_general(3), expected.to_bits());
            assert_eq!(state.read_cp1_control(31), 0);
        }

        let cases = [
            (Cp1BinaryOperation::Add, 7.0_f64, 2.0_f64, 9.0_f64),
            (Cp1BinaryOperation::Subtract, 7.0, 2.0, 5.0),
            (Cp1BinaryOperation::Multiply, 7.0, 2.0, 14.0),
            (Cp1BinaryOperation::Divide, 7.0, 2.0, 3.5),
        ];

        for (operation, lhs, rhs, expected) in cases {
            let mut state = enabled_state();
            state
                .cp1_mut()
                .write_float64(1, Float64::from_bits(lhs.to_bits()));
            state
                .cp1_mut()
                .write_float64(3, Float64::from_bits(rhs.to_bits()));

            run(
                &mut state,
                binary(operation, Cp1FloatFormat::Double, 3, 1, 5),
            );

            assert_eq!(state.cp1().read_float64(5).to_bits(), expected.to_bits());
            assert_eq!(state.read_cp1_control(31), 0);
        }
    }

    #[test]
    fn unary_operations_and_register_layout_follow_r3010_rules() {
        let mut state = enabled_state();
        state.cp1_mut().write_general_register(6, 0xaaaa_aaaa);
        state
            .cp1_mut()
            .write_float32(1, Float32::from_bits((-3.0_f32).to_bits()));
        run(
            &mut state,
            Cp1Instruction::Unary {
                operation: Cp1UnaryOperation::Absolute,
                format: Cp1FloatFormat::Single,
                fs: 1,
                fd: 5,
            },
        );
        assert_eq!(state.read_cp1_general(5), 3.0_f32.to_bits());
        assert_eq!(state.read_cp1_general(6), 0xaaaa_aaaa);

        state
            .cp1_mut()
            .write_float32(1, Float32::from_bits(3.0_f32.to_bits()));
        run(
            &mut state,
            Cp1Instruction::Unary {
                operation: Cp1UnaryOperation::Negate,
                format: Cp1FloatFormat::Single,
                fs: 1,
                fd: 7,
            },
        );
        assert_eq!(state.read_cp1_general(7), (-3.0_f32).to_bits());

        state
            .cp1_mut()
            .write_float64(3, Float64::from_bits((-2.0_f64).to_bits()));
        run(
            &mut state,
            Cp1Instruction::Unary {
                operation: Cp1UnaryOperation::Absolute,
                format: Cp1FloatFormat::Double,
                fs: 3,
                fd: 9,
            },
        );
        assert_eq!(state.cp1().read_float64(9).to_bits(), 2.0_f64.to_bits());

        state
            .cp1_mut()
            .write_float64(3, Float64::from_bits(2.0_f64.to_bits()));
        run(
            &mut state,
            Cp1Instruction::Unary {
                operation: Cp1UnaryOperation::Negate,
                format: Cp1FloatFormat::Double,
                fs: 3,
                fd: 9,
            },
        );
        assert_eq!(state.cp1().read_float64(9).to_bits(), (-2.0_f64).to_bits());

        let preserved_status = (1 << 17) | (1 << 16) | (1 << 6) | 3;
        let preserved_eir = 0x1234_5678;
        state.cp1_mut().write_control_register(30, preserved_eir);
        state.cp1_mut().write_control_register(31, preserved_status);
        state.cp1_mut().write_general_register(12, 0x7fc0_0000);
        run(
            &mut state,
            Cp1Instruction::Unary {
                operation: Cp1UnaryOperation::Move,
                format: Cp1FloatFormat::Single,
                fs: 12,
                fd: 13,
            },
        );
        assert_eq!(state.read_cp1_general(13), 0x7fc0_0000);
        assert_eq!(state.read_cp1_control(31), preserved_status);

        let double_bits = 0x7ff7_ffff_1234_5678;
        state
            .cp1_mut()
            .write_float64(15, Float64::from_bits(double_bits));
        run(
            &mut state,
            Cp1Instruction::Unary {
                operation: Cp1UnaryOperation::Move,
                format: Cp1FloatFormat::Double,
                fs: 15,
                fd: 21,
            },
        );
        assert_eq!(state.cp1().read_float64(21).to_bits(), double_bits);
        assert_eq!(state.read_cp1_control(30), preserved_eir);
        assert_eq!(state.read_cp1_control(31), preserved_status);
    }

    #[test]
    fn infinity_and_signed_zero_complete_when_the_operation_is_defined() {
        let mut state = enabled_state();
        state
            .cp1_mut()
            .write_float32(1, Float32::from_bits(f32::INFINITY.to_bits()));
        state
            .cp1_mut()
            .write_float32(2, Float32::from_bits(1.0_f32.to_bits()));
        run(
            &mut state,
            binary(Cp1BinaryOperation::Add, Cp1FloatFormat::Single, 2, 1, 3),
        );
        assert_eq!(state.read_cp1_general(3), f32::INFINITY.to_bits());

        run(
            &mut state,
            binary(Cp1BinaryOperation::Divide, Cp1FloatFormat::Single, 1, 2, 3),
        );
        assert_eq!(state.read_cp1_general(3), 0.0_f32.to_bits());

        state
            .cp1_mut()
            .write_float32(1, Float32::from_bits(0.0_f32.to_bits()));
        run(
            &mut state,
            Cp1Instruction::Unary {
                operation: Cp1UnaryOperation::Negate,
                format: Cp1FloatFormat::Single,
                fs: 1,
                fd: 3,
            },
        );
        assert_eq!(state.read_cp1_general(3), (-0.0_f32).to_bits());

        state
            .cp1_mut()
            .write_float64(5, Float64::from_bits((-0.0_f64).to_bits()));
        run(
            &mut state,
            Cp1Instruction::Unary {
                operation: Cp1UnaryOperation::Absolute,
                format: Cp1FloatFormat::Double,
                fs: 5,
                fd: 7,
            },
        );
        assert_eq!(state.cp1().read_float64(7).to_bits(), 0.0_f64.to_bits());

        state
            .cp1_mut()
            .write_float32(1, Float32::from_bits(f32::NEG_INFINITY.to_bits()));
        run(
            &mut state,
            Cp1Instruction::Convert {
                operation: Cp1Conversion::SingleToDouble,
                fs: 1,
                fd: 9,
            },
        );
        assert_eq!(
            state.cp1().read_float64(9).to_bits(),
            f64::NEG_INFINITY.to_bits()
        );

        state
            .cp1_mut()
            .write_float64(1, Float64::from_bits(f64::NEG_INFINITY.to_bits()));
        state
            .cp1_mut()
            .write_float64(3, Float64::from_bits(1.0_f64.to_bits()));
        run(
            &mut state,
            Cp1Instruction::Compare {
                format: Cp1FloatFormat::Double,
                condition: 0x4,
                fs: 1,
                ft: 3,
            },
        );
        state.complete_instruction(None, None);
        assert!(state.cp1_condition());
        assert_eq!(
            state.read_cp1_control(31) & (CONTROL_STATUS_CAUSE_MASK | CONTROL_STATUS_UNIMPLEMENTED),
            0
        );
    }

    #[test]
    fn conversions_cover_all_source_and_destination_formats() {
        let cases = [
            (
                Cp1Conversion::SingleToDouble,
                1.5_f32.to_bits(),
                1.5_f64.to_bits(),
            ),
            (
                Cp1Conversion::WordToDouble,
                (-4_i32) as u32,
                (-4.0_f64).to_bits(),
            ),
        ];
        for (operation, source, expected) in cases {
            let mut state = enabled_state();
            state.cp1_mut().write_general_register(3, source);
            run(
                &mut state,
                Cp1Instruction::Convert {
                    operation,
                    fs: 3,
                    fd: 5,
                },
            );
            assert_eq!(state.cp1().read_float64(5).to_bits(), expected);
        }

        let cases = [
            (Cp1Conversion::DoubleToSingle, 1.5_f64, 1.5_f32.to_bits()),
            (Cp1Conversion::DoubleToWord, -2.0_f64, (-2_i32) as u32),
        ];
        for (operation, source, expected) in cases {
            let mut state = enabled_state();
            state
                .cp1_mut()
                .write_float64(3, Float64::from_bits(source.to_bits()));
            run(
                &mut state,
                Cp1Instruction::Convert {
                    operation,
                    fs: 3,
                    fd: 7,
                },
            );
            assert_eq!(state.read_cp1_general(7), expected);
        }

        let cases = [
            (
                Cp1Conversion::WordToSingle,
                (-3_i32) as u32,
                (-3.0_f32).to_bits(),
            ),
            (Cp1Conversion::SingleToWord, 2.0_f32.to_bits(), 2_u32),
        ];
        for (operation, source, expected) in cases {
            let mut state = enabled_state();
            state.cp1_mut().write_general_register(3, source);
            state.cp1_mut().write_general_register(8, 0xaaaa_aaaa);
            run(
                &mut state,
                Cp1Instruction::Convert {
                    operation,
                    fs: 3,
                    fd: 7,
                },
            );
            assert_eq!(state.read_cp1_general(7), expected);
            assert_eq!(state.read_cp1_general(8), 0xaaaa_aaaa);
        }
    }

    #[test]
    fn conversion_rounding_uses_fcr31_rm() {
        let halfway = 1.0_f64 + 2.0_f64.powi(-24);
        let negative_halfway = -halfway;
        let float_cases = [
            (0_u32, 1.0_f32.to_bits(), (-1.0_f32).to_bits()),
            (1, 1.0_f32.to_bits(), (-1.0_f32).to_bits()),
            (2, 0x3f80_0001, (-1.0_f32).to_bits()),
            (3, 1.0_f32.to_bits(), 0xbf80_0001),
        ];

        for (rm, positive, negative) in float_cases {
            for (source, expected) in [(halfway, positive), (negative_halfway, negative)] {
                let mut state = enabled_state();
                state.cp1_mut().write_control_register(31, rm);
                state
                    .cp1_mut()
                    .write_float64(2, Float64::from_bits(source.to_bits()));
                run(
                    &mut state,
                    Cp1Instruction::Convert {
                        operation: Cp1Conversion::DoubleToSingle,
                        fs: 2,
                        fd: 4,
                    },
                );
                assert_eq!(state.read_cp1_general(4), expected);
                assert_eq!(state.read_cp1_control(31) & 3, rm);
                assert_ne!(
                    state.read_cp1_control(31)
                        & exception_bits(ExceptionFlags::INEXACT, FCR31_CAUSE_SHIFT),
                    0
                );
            }
        }

        let word_cases = [
            (RoundingMode::NearestEven, 0_u32, 2_u32, (-2_i32) as u32),
            (RoundingMode::TowardZero, 1, 1, (-1_i32) as u32),
            (RoundingMode::TowardPositive, 2, 2, (-1_i32) as u32),
            (RoundingMode::TowardNegative, 3, 1, (-2_i32) as u32),
        ];
        for (_mode, rm, positive, negative) in word_cases {
            for (source, expected) in [(1.5_f64, positive), (-1.5_f64, negative)] {
                let mut state = enabled_state();
                state.cp1_mut().write_control_register(31, rm);
                state
                    .cp1_mut()
                    .write_float64(2, Float64::from_bits(source.to_bits()));
                run(
                    &mut state,
                    Cp1Instruction::Convert {
                        operation: Cp1Conversion::DoubleToWord,
                        fs: 2,
                        fd: 4,
                    },
                );
                assert_eq!(state.read_cp1_general(4), expected);
            }
        }
    }

    #[test]
    fn clean_operations_clear_current_exception_state_and_preserve_sticky_flags() {
        let mut state = enabled_state();
        let sticky = exception_bits(ExceptionFlags::INVALID, FCR31_FLAG_SHIFT);
        let initial = sticky
            | CONTROL_STATUS_CAUSE_MASK
            | CONTROL_STATUS_UNIMPLEMENTED
            | CONTROL_STATUS_CONDITION
            | 2;
        state.cp1_mut().write_control_register(31, initial);
        state
            .cp1_mut()
            .write_float32(1, Float32::from_bits(1.0_f32.to_bits()));
        state
            .cp1_mut()
            .write_float32(2, Float32::from_bits(2.0_f32.to_bits()));

        run(
            &mut state,
            binary(Cp1BinaryOperation::Add, Cp1FloatFormat::Single, 2, 1, 3),
        );

        assert_eq!(state.read_cp1_general(3), 3.0_f32.to_bits());
        assert_eq!(
            state.read_cp1_control(31),
            sticky | CONTROL_STATUS_CONDITION | 2
        );
        assert!(!state.cp1_interrupt_asserted());
    }

    #[test]
    fn inexact_and_overflow_follow_cause_flag_and_enable_rules() {
        let inexact = ExceptionFlags::INEXACT;
        let inexact_cause = exception_bits(inexact, FCR31_CAUSE_SHIFT);
        let inexact_flag = exception_bits(inexact, FCR31_FLAG_SHIFT);

        let mut unenabled = enabled_state();
        unenabled
            .cp1_mut()
            .write_general_register(1, 16_777_217_u32);
        run(
            &mut unenabled,
            Cp1Instruction::Convert {
                operation: Cp1Conversion::WordToSingle,
                fs: 1,
                fd: 2,
            },
        );
        assert_eq!(unenabled.read_cp1_general(2), (16_777_216_f32).to_bits());
        assert_eq!(
            unenabled.read_cp1_control(31) & (CONTROL_STATUS_CAUSE_MASK | FCR31_FLAG_MASK),
            inexact_cause | inexact_flag
        );

        let old_flag = exception_bits(ExceptionFlags::OVERFLOW, FCR31_FLAG_SHIFT);
        let mut enabled = enabled_state();
        enabled
            .cp1_mut()
            .write_control_register(31, old_flag | exception_bits(inexact, FCR31_ENABLE_SHIFT));
        enabled.cp1_mut().write_general_register(1, 16_777_217_u32);
        enabled.cp1_mut().write_general_register(2, 0xaaaa_aaaa);
        run(
            &mut enabled,
            Cp1Instruction::Convert {
                operation: Cp1Conversion::WordToSingle,
                fs: 1,
                fd: 2,
            },
        );
        assert_eq!(enabled.read_cp1_general(2), 0xaaaa_aaaa);
        assert_eq!(enabled.read_cp1_control(31) & FCR31_FLAG_MASK, old_flag);
        assert_eq!(
            enabled.read_cp1_control(31) & CONTROL_STATUS_CAUSE_MASK,
            inexact_cause
        );
        assert!(enabled.cp1_interrupt_asserted());

        let overflow_flags = ExceptionFlags::OVERFLOW | ExceptionFlags::INEXACT;
        for enabled_exception in [ExceptionFlags::empty(), ExceptionFlags::OVERFLOW, inexact] {
            let mut state = enabled_state();
            state
                .cp1_mut()
                .write_control_register(31, exception_bits(enabled_exception, FCR31_ENABLE_SHIFT));
            state.cp1_mut().write_general_register(1, 0x7f7f_ffff);
            state
                .cp1_mut()
                .write_float32(2, Float32::from_bits(2.0_f32.to_bits()));
            state.cp1_mut().write_general_register(3, 0xaaaa_aaaa);
            run(
                &mut state,
                binary(
                    Cp1BinaryOperation::Multiply,
                    Cp1FloatFormat::Single,
                    2,
                    1,
                    3,
                ),
            );

            assert_eq!(
                state.read_cp1_control(31) & CONTROL_STATUS_CAUSE_MASK,
                exception_bits(overflow_flags, FCR31_CAUSE_SHIFT)
            );
            if enabled_exception.is_empty() {
                assert_eq!(state.read_cp1_general(3), f32::INFINITY.to_bits());
                assert_eq!(
                    state.read_cp1_control(31) & FCR31_FLAG_MASK,
                    exception_bits(overflow_flags, FCR31_FLAG_SHIFT)
                );
            } else {
                assert_eq!(state.read_cp1_general(3), 0xaaaa_aaaa);
                assert_eq!(state.read_cp1_control(31) & FCR31_FLAG_MASK, 0);
                assert!(state.cp1_interrupt_asserted());
            }
        }
    }

    #[test]
    fn r3010_unimplemented_conditions_preserve_destination_flags_and_feir() {
        let old_flag = exception_bits(ExceptionFlags::INEXACT, FCR31_FLAG_SHIFT);

        let mut unsupported = enabled_state();
        assert_unimplemented_result(
            &mut unsupported,
            Cp1Instruction::UnimplementedOperation,
            old_flag,
            20,
        );

        for source in [0x0000_0001, 0x7fbf_ffff, 0x7fc0_0000] {
            let mut state = enabled_state();
            state.cp1_mut().write_general_register(1, source);
            state
                .cp1_mut()
                .write_float32(2, Float32::from_bits(1.0_f32.to_bits()));
            assert_unimplemented_result(
                &mut state,
                binary(Cp1BinaryOperation::Add, Cp1FloatFormat::Single, 2, 1, 20),
                old_flag,
                20,
            );
        }

        for (lhs, rhs, operation) in [
            (0.0_f32, 0.0_f32, Cp1BinaryOperation::Divide),
            (1.0_f32, 0.0_f32, Cp1BinaryOperation::Divide),
            (f32::INFINITY, f32::INFINITY, Cp1BinaryOperation::Subtract),
            (
                f32::from_bits(0x0080_0000),
                0.5_f32,
                Cp1BinaryOperation::Multiply,
            ),
        ] {
            let mut state = enabled_state();
            state
                .cp1_mut()
                .write_float32(1, Float32::from_bits(lhs.to_bits()));
            state
                .cp1_mut()
                .write_float32(2, Float32::from_bits(rhs.to_bits()));
            assert_unimplemented_result(
                &mut state,
                binary(operation, Cp1FloatFormat::Single, 2, 1, 20),
                old_flag,
                20,
            );
        }

        let mut conversion = enabled_state();
        conversion.cp1_mut().write_general_register(1, 0x7f7f_ffff);
        assert_unimplemented_result(
            &mut conversion,
            Cp1Instruction::Convert {
                operation: Cp1Conversion::SingleToWord,
                fs: 1,
                fd: 20,
            },
            old_flag,
            20,
        );
    }

    #[test]
    fn invalid_enable_selects_ieee_invalid_instead_of_unimplemented() {
        let invalid_enable = exception_bits(ExceptionFlags::INVALID, FCR31_ENABLE_SHIFT);
        let invalid_cause = exception_bits(ExceptionFlags::INVALID, FCR31_CAUSE_SHIFT);
        let old_flag = exception_bits(ExceptionFlags::INEXACT, FCR31_FLAG_SHIFT);

        for (lhs, rhs) in [(f32::from_bits(0x7fc0_0000), 1.0_f32), (0.0, 0.0)] {
            let mut state = enabled_state();
            state
                .cp1_mut()
                .write_control_register(31, invalid_enable | old_flag);
            state
                .cp1_mut()
                .write_float32(1, Float32::from_bits(lhs.to_bits()));
            state
                .cp1_mut()
                .write_float32(2, Float32::from_bits(rhs.to_bits()));
            state.cp1_mut().write_general_register(3, 0xaaaa_aaaa);
            run(
                &mut state,
                binary(
                    if rhs == 0.0 {
                        Cp1BinaryOperation::Divide
                    } else {
                        Cp1BinaryOperation::Add
                    },
                    Cp1FloatFormat::Single,
                    2,
                    1,
                    3,
                ),
            );

            assert_eq!(state.read_cp1_general(3), 0xaaaa_aaaa);
            assert_eq!(
                state.read_cp1_control(31) & CONTROL_STATUS_CAUSE_MASK,
                invalid_cause
            );
            assert_eq!(state.read_cp1_control(31) & FCR31_FLAG_MASK, old_flag);
            assert_eq!(state.read_cp1_control(31) & CONTROL_STATUS_UNIMPLEMENTED, 0);
            assert!(state.cp1_interrupt_asserted());
        }

        let mut quiet_nan = enabled_state();
        quiet_nan.cp1_mut().write_general_register(1, 0x7fbf_ffff);
        quiet_nan
            .cp1_mut()
            .write_float32(2, Float32::from_bits(1.0_f32.to_bits()));
        assert_unimplemented_result(
            &mut quiet_nan,
            binary(Cp1BinaryOperation::Add, Cp1FloatFormat::Single, 2, 1, 20),
            invalid_enable | old_flag,
            20,
        );

        let mut subnormal_precedence = enabled_state();
        subnormal_precedence
            .cp1_mut()
            .write_general_register(1, 0x7fc0_0000);
        subnormal_precedence
            .cp1_mut()
            .write_general_register(2, 0x0000_0001);
        assert_unimplemented_result(
            &mut subnormal_precedence,
            binary(Cp1BinaryOperation::Add, Cp1FloatFormat::Single, 2, 1, 20),
            invalid_enable | old_flag,
            20,
        );
    }

    #[test]
    fn divide_by_zero_and_underflow_enables_do_not_override_unimplemented() {
        let old_flag = exception_bits(ExceptionFlags::OVERFLOW, FCR31_FLAG_SHIFT);
        let cases = [
            (
                1.0_f32,
                0.0_f32,
                Cp1BinaryOperation::Divide,
                ExceptionFlags::DIVIDE_BY_ZERO,
            ),
            (
                f32::from_bits(0x0080_0000),
                0.5_f32,
                Cp1BinaryOperation::Multiply,
                ExceptionFlags::UNDERFLOW,
            ),
        ];

        for (lhs, rhs, operation, enabled_exception) in cases {
            let mut state = enabled_state();
            state
                .cp1_mut()
                .write_float32(1, Float32::from_bits(lhs.to_bits()));
            state
                .cp1_mut()
                .write_float32(2, Float32::from_bits(rhs.to_bits()));
            assert_unimplemented_result(
                &mut state,
                binary(operation, Cp1FloatFormat::Single, 2, 1, 20),
                old_flag | exception_bits(enabled_exception, FCR31_ENABLE_SHIFT),
                20,
            );
        }
    }

    #[test]
    fn all_compare_conditions_use_the_relation_mask_and_one_instruction_delay() {
        for format in [Cp1FloatFormat::Single, Cp1FloatFormat::Double] {
            for (lhs, rhs, relation_bit) in
                [(1.0_f64, 2.0_f64, 0x4), (2.0, 2.0, 0x2), (3.0, 2.0, 0)]
            {
                for condition in 0..=0x0f {
                    let expected = condition & relation_bit != 0;
                    let mut state = enabled_state();
                    state.cp1_mut().write_condition(!expected);
                    match format {
                        Cp1FloatFormat::Single => {
                            state
                                .cp1_mut()
                                .write_float32(1, Float32::from_bits((lhs as f32).to_bits()));
                            state
                                .cp1_mut()
                                .write_float32(2, Float32::from_bits((rhs as f32).to_bits()));
                        }
                        Cp1FloatFormat::Double => {
                            state
                                .cp1_mut()
                                .write_float64(1, Float64::from_bits(lhs.to_bits()));
                            state
                                .cp1_mut()
                                .write_float64(3, Float64::from_bits(rhs.to_bits()));
                        }
                    }
                    let ft = if format == Cp1FloatFormat::Single {
                        2
                    } else {
                        3
                    };

                    run(
                        &mut state,
                        Cp1Instruction::Compare {
                            format,
                            condition,
                            fs: 1,
                            ft,
                        },
                    );

                    assert_eq!(state.cp1_condition(), !expected);
                    assert_eq!(
                        state.read_cp1_control(31)
                            & (CONTROL_STATUS_CAUSE_MASK | CONTROL_STATUS_UNIMPLEMENTED),
                        0
                    );
                    state.complete_instruction(None, None);
                    assert_eq!(state.cp1_condition(), expected);
                }
            }
        }
    }

    #[test]
    fn compare_special_values_do_not_schedule_condition_updates() {
        let invalid_enable = exception_bits(ExceptionFlags::INVALID, FCR31_ENABLE_SHIFT);
        let cases = [
            (0x7fbf_ffff, 0x01_u8, 0_u32, true),
            (0x7fc0_0000, 0x01, invalid_enable, false),
            (0x7fbf_ffff, 0x09, invalid_enable, false),
            (0x0000_0001, 0x04, 0, true),
        ];

        for (source, condition, status, unimplemented) in cases {
            let mut state = enabled_state();
            state.cp1_mut().write_control_register(31, status);
            state.cp1_mut().write_condition(false);
            state.cp1_mut().write_general_register(1, source);
            state
                .cp1_mut()
                .write_float32(2, Float32::from_bits(1.0_f32.to_bits()));
            run(
                &mut state,
                Cp1Instruction::Compare {
                    format: Cp1FloatFormat::Single,
                    condition,
                    fs: 1,
                    ft: 2,
                },
            );
            state.complete_instruction(None, None);

            assert!(!state.cp1_condition());
            assert_eq!(
                state.read_cp1_control(31) & CONTROL_STATUS_UNIMPLEMENTED != 0,
                unimplemented
            );
            if unimplemented {
                assert_eq!(state.read_cp1_control(31) & CONTROL_STATUS_CAUSE_MASK, 0);
            } else {
                assert_eq!(
                    state.read_cp1_control(31) & CONTROL_STATUS_CAUSE_MASK,
                    exception_bits(ExceptionFlags::INVALID, FCR31_CAUSE_SHIFT)
                );
            }
        }
    }

    #[test]
    fn consecutive_comparisons_commit_conditions_in_order() {
        let mut state = enabled_state();
        state
            .cp1_mut()
            .write_float32(1, Float32::from_bits(1.0_f32.to_bits()));
        state
            .cp1_mut()
            .write_float32(2, Float32::from_bits(2.0_f32.to_bits()));
        state
            .cp1_mut()
            .write_float32(3, Float32::from_bits(3.0_f32.to_bits()));

        run(
            &mut state,
            Cp1Instruction::Compare {
                format: Cp1FloatFormat::Single,
                condition: 0x4,
                fs: 1,
                ft: 2,
            },
        );
        assert!(!state.cp1_condition());

        run(
            &mut state,
            Cp1Instruction::Compare {
                format: Cp1FloatFormat::Single,
                condition: 0x4,
                fs: 3,
                ft: 2,
            },
        );
        assert!(state.cp1_condition());

        state.complete_instruction(None, None);
        assert!(!state.cp1_condition());
    }

    #[test]
    fn arithmetic_observes_old_operands_then_overrides_pending_cp1_writes() {
        let mut state = enabled_state();
        state
            .cp1_mut()
            .write_float32(1, Float32::from_bits(1.0_f32.to_bits()));
        state
            .cp1_mut()
            .write_float32(2, Float32::from_bits(2.0_f32.to_bits()));
        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp1GeneralWrite {
                index: 1,
                value: 10.0_f32.to_bits(),
            }),
        );

        run(
            &mut state,
            binary(Cp1BinaryOperation::Add, Cp1FloatFormat::Single, 2, 1, 1),
        );

        assert_eq!(state.read_cp1_general(1), 3.0_f32.to_bits());
    }

    #[test]
    fn arithmetic_samples_old_rm_then_updates_the_committed_fcr31() {
        let mut state = enabled_state();
        let halfway = 1.0_f64 + 2.0_f64.powi(-24);
        state
            .cp1_mut()
            .write_float64(2, Float64::from_bits(halfway.to_bits()));
        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp1ControlWrite {
                index: 31,
                value: 2 | CONTROL_STATUS_CAUSE_MASK | CONTROL_STATUS_UNIMPLEMENTED,
            }),
        );

        run(
            &mut state,
            Cp1Instruction::Convert {
                operation: Cp1Conversion::DoubleToSingle,
                fs: 2,
                fd: 4,
            },
        );

        assert_eq!(state.read_cp1_general(4), 1.0_f32.to_bits());
        let status = state.read_cp1_control(31);
        assert_eq!(status & 3, 2);
        assert_eq!(
            status & CONTROL_STATUS_CAUSE_MASK,
            exception_bits(ExceptionFlags::INEXACT, FCR31_CAUSE_SHIFT)
        );
        assert_eq!(status & CONTROL_STATUS_UNIMPLEMENTED, 0);
    }

    #[test]
    fn unimplemented_operation_commits_an_older_cp1_write() {
        let mut state = enabled_state();
        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp1GeneralWrite {
                index: 5,
                value: 0x1234_5678,
            }),
        );

        run(&mut state, Cp1Instruction::UnimplementedOperation);

        assert_eq!(state.read_cp1_general(5), 0x1234_5678);
        assert!(state.cp1_interrupt_asserted());
    }

    #[test]
    fn native_backend_executes_finite_values_and_keeps_r3010_prechecks() {
        let config = R3000Config::new(4 * 1024, 4 * 1024, 4, 4, true, Backend::Native);
        let mut state = State::new(config);
        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp0Write {
                index: 12,
                value: state.read_cp0(12) | STATUS_CU1,
            }),
        );
        state.complete_instruction(None, None);
        state.complete_instruction(None, None);
        state
            .cp1_mut()
            .write_float32(1, Float32::from_bits(1.0_f32.to_bits()));
        state
            .cp1_mut()
            .write_float32(2, Float32::from_bits(2.0_f32.to_bits()));

        run(
            &mut state,
            binary(Cp1BinaryOperation::Add, Cp1FloatFormat::Single, 2, 1, 3),
        );
        assert_eq!(state.read_cp1_general(3), 3.0_f32.to_bits());

        state.cp1_mut().write_general_register(5, (-4_i32) as u32);
        run(
            &mut state,
            Cp1Instruction::Convert {
                operation: Cp1Conversion::WordToDouble,
                fs: 5,
                fd: 7,
            },
        );
        assert_eq!(state.cp1().read_float64(7).to_bits(), (-4.0_f64).to_bits());

        let preserved_status = CONTROL_STATUS_UNIMPLEMENTED | CONTROL_STATUS_CONDITION | 3;
        state.cp1_mut().write_control_register(31, preserved_status);
        state.cp1_mut().write_general_register(8, 0x7fc0_0000);
        run(
            &mut state,
            Cp1Instruction::Unary {
                operation: Cp1UnaryOperation::Move,
                format: Cp1FloatFormat::Single,
                fs: 8,
                fd: 9,
            },
        );
        assert_eq!(state.read_cp1_general(9), 0x7fc0_0000);
        assert_eq!(state.read_cp1_control(31), preserved_status);

        state.cp1_mut().write_general_register(1, 1);
        assert_unimplemented_result(
            &mut state,
            binary(Cp1BinaryOperation::Add, Cp1FloatFormat::Single, 2, 1, 4),
            0,
            4,
        );
    }
}
