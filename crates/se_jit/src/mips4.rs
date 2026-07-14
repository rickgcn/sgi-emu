//! Cranelift backend for typed MIPS IV basic blocks.

use core::{fmt, mem, ptr::NonNull};

use cranelift_codegen::ir::{
    AbiParam, Function, InstBuilder, MemFlagsData, Signature, Value, condcodes::IntCC, types,
};
use cranelift_codegen::isa::CallConv;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Switch};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module, default_libcall_names};
use se_device::cpu::mips4::execution::block::{
    MIPS4_BLOCK_FRAME_BUDGET_OFFSET, MIPS4_BLOCK_FRAME_DELAY_PC_OFFSET,
    MIPS4_BLOCK_FRAME_DELAY_VALID_OFFSET, MIPS4_BLOCK_FRAME_EXCEPTION_OFFSET,
    MIPS4_BLOCK_FRAME_GPR_WRITE_THROUGH_OFFSET, MIPS4_BLOCK_FRAME_HI_OFFSET,
    MIPS4_BLOCK_FRAME_LO_OFFSET, MIPS4_BLOCK_FRAME_NEXT_PC_OFFSET,
    MIPS4_BLOCK_FRAME_OPERATIONS_EXECUTED_OFFSET, MIPS4_BLOCK_FRAME_PC_OFFSET,
    MIPS4_BLOCK_FRAME_RETIRED_OFFSET, MIPS4_BLOCK_FRAME_RUNTIME_CALL_OFFSET,
    MIPS4_BLOCK_FRAME_RUNTIME_CALLS_OFFSET, MIPS4_BLOCK_FRAME_RUNTIME_CONTEXT_OFFSET, Mips4Block,
    Mips4BlockArithmetic, Mips4BlockBranchCondition, Mips4BlockBranchTarget, Mips4BlockComparison,
    Mips4BlockException, Mips4BlockExit, Mips4BlockFrame, Mips4BlockLogical, Mips4BlockOperand,
    Mips4BlockOperation, Mips4BlockShift, Mips4BlockShiftAmount, Mips4BlockTrap, Mips4BlockWidth,
    Mips4CodegenBackend, Mips4RuntimeOperation, Mips4RuntimeResult, mips4_block_frame_gpr_offset,
};

/// Compiled host entry point owned by one Cranelift module generation.
#[derive(Debug)]
pub struct CraneliftMips4Block {
    entry: NonNull<u8>,
}

/// Cranelift construction, compilation, or execution failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CraneliftMips4Error {
    message: String,
}

impl CraneliftMips4Error {
    fn new(error: impl fmt::Display) -> Self {
        Self {
            message: error.to_string(),
        }
    }

    fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for CraneliftMips4Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CraneliftMips4Error {}

/// Native MIPS IV block backend using the current host ISA.
pub struct CraneliftMips4Backend {
    module: JITModule,
    next_function: u64,
}

impl CraneliftMips4Backend {
    /// Creates an empty native backend for the current host.
    pub fn new() -> Result<Self, CraneliftMips4Error> {
        Ok(Self {
            module: create_module()?,
            next_function: 0,
        })
    }
}

impl Mips4CodegenBackend for CraneliftMips4Backend {
    type CompiledBlock = CraneliftMips4Block;
    type Error = CraneliftMips4Error;

    fn compile(&mut self, block: &Mips4Block) -> Result<Self::CompiledBlock, Self::Error> {
        block.verify().map_err(CraneliftMips4Error::new)?;
        let mut context = self.module.make_context();
        let pointer_type = self.module.target_config().pointer_type();
        context.func = Function::with_name_signature(
            cranelift_codegen::ir::UserFuncName::user(0, self.next_function as u32),
            Signature {
                params: vec![AbiParam::new(pointer_type)],
                returns: vec![AbiParam::new(types::I32)],
                call_conv: self.module.target_config().default_call_conv,
            },
        );

        let mut function_builder_context = FunctionBuilderContext::new();
        {
            let mut builder =
                FunctionBuilder::new(&mut context.func, &mut function_builder_context);
            let entry = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);
            builder.seal_block(entry);
            let frame = builder.block_params(entry)[0];
            lower_block(
                &mut builder,
                frame,
                block,
                pointer_type,
                self.module.target_config().default_call_conv,
            );
            builder.seal_all_blocks();
            builder.finalize();
        }

        let name = format!("mips4_block_{}", self.next_function);
        self.next_function = self.next_function.wrapping_add(1);
        let function = self
            .module
            .declare_function(&name, Linkage::Local, &context.func.signature)
            .map_err(CraneliftMips4Error::new)?;
        self.module
            .define_function(function, &mut context)
            .map_err(CraneliftMips4Error::new)?;
        self.module.clear_context(&mut context);
        self.module
            .finalize_definitions()
            .map_err(CraneliftMips4Error::new)?;
        let entry = NonNull::new(self.module.get_finalized_function(function).cast_mut())
            .ok_or_else(|| CraneliftMips4Error::message("Cranelift returned a null entry point"))?;
        Ok(CraneliftMips4Block { entry })
    }

    fn execute(
        &mut self,
        compiled: &Self::CompiledBlock,
        frame: &mut Mips4BlockFrame,
    ) -> Result<Mips4BlockExit, Self::Error> {
        type NativeBlock = unsafe extern "C" fn(*mut Mips4BlockFrame) -> u32;

        let installed_write_through = frame.prepare_native_gpr_write_through();
        // SAFETY: Cranelift emitted this entry with the exact NativeBlock signature,
        // and the module remains alive while every compiled handle is cached.
        let function: NativeBlock = unsafe { mem::transmute(compiled.entry.as_ptr()) };
        // SAFETY: The frame is uniquely borrowed for the duration of native execution.
        let code = unsafe { function(frame) };
        frame.release_native_gpr_write_through(installed_write_through);
        Mips4BlockExit::from_code(code).ok_or_else(|| {
            CraneliftMips4Error::message(format!("native MIPS IV block returned exit code {code}"))
        })
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.module = create_module()?;
        self.next_function = 0;
        Ok(())
    }
}

fn create_module() -> Result<JITModule, CraneliftMips4Error> {
    let mut settings_builder = settings::builder();
    settings_builder
        .set("opt_level", "speed")
        .map_err(CraneliftMips4Error::new)?;
    let flags = settings::Flags::new(settings_builder);
    let isa_builder = cranelift_native::builder().map_err(CraneliftMips4Error::new)?;
    let isa = isa_builder
        .finish(flags)
        .map_err(CraneliftMips4Error::new)?;
    Ok(JITModule::new(JITBuilder::with_isa(
        isa,
        default_libcall_names(),
    )))
}

fn lower_block(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    block: &Mips4Block,
    pointer_type: cranelift_codegen::ir::Type,
    call_conv: CallConv,
) {
    let mut runtime_index = 0;
    let mut entered_operations = 0_u64;
    let mut control = load_control(builder, frame);
    let mut accounting = load_accounting(builder, frame);
    let runtime_abi = NativeRuntimeAbi {
        pointer_type,
        call_conv,
    };
    for instruction in block.body() {
        entered_operations += 1;
        if operation_requires_early_count(instruction.operation) {
            lower_enter_operation(builder, frame, entered_operations);
            store_control(builder, frame, control);
        }
        let runtime_operation = matches!(instruction.operation, Mips4BlockOperation::Runtime(_));
        match lower_operation(
            builder,
            frame,
            instruction.operation,
            &mut runtime_index,
            entered_operations,
            &mut accounting,
            runtime_abi,
        ) {
            LoweredOperation::NeedsRetirement => {
                retire_control_sequential(builder, &mut control);
                lower_budget_check_with_control(
                    builder,
                    frame,
                    entered_operations,
                    control,
                    &mut accounting,
                );
            }
            LoweredOperation::Retired => {
                if runtime_operation {
                    control = load_control(builder, frame);
                }
            }
            LoweredOperation::Terminated => return,
        }
    }

    let Some(branch) = block.branch() else {
        lower_enter_operation(builder, frame, entered_operations);
        store_accounting(builder, frame, accounting);
        store_control(builder, frame, control);
        return_exit(builder, Mips4BlockExit::Dispatch);
        return;
    };
    entered_operations += 1;
    let target = match branch.target {
        Mips4BlockBranchTarget::Direct(target) => builder.ins().iconst(types::I64, target as i64),
        Mips4BlockBranchTarget::Register(register) => {
            lower_enter_operation(builder, frame, entered_operations);
            store_control(builder, frame, control);
            let target = load_gpr(builder, frame, register);
            let low = builder.ins().band_imm(target, 3);
            let misaligned = builder.ins().icmp_imm(IntCC::NotEqual, low, 0);
            exception_if(
                builder,
                frame,
                misaligned,
                Mips4BlockException::AddressErrorLoad,
                accounting,
            );
            target
        }
    };
    if let Some(link) = branch.link {
        let value = builder
            .ins()
            .iconst(types::I64, branch.metadata.pc.wrapping_add(8) as i64);
        store_gpr(builder, frame, link, value);
    }
    let taken = lower_branch_condition(builder, frame, branch.condition);
    let nullify = if branch.likely {
        builder.ins().icmp_imm(IntCC::Equal, taken, 0)
    } else {
        builder.ins().iconst(types::I8, 0)
    };
    retire_branch_control(
        builder,
        &mut control,
        branch.metadata.pc,
        target,
        taken,
        nullify,
    );
    lower_budget_check_with_control(builder, frame, entered_operations, control, &mut accounting);

    let dispatch = builder.create_block();
    let delay = builder.create_block();
    builder.ins().brif(nullify, dispatch, &[], delay, &[]);
    builder.switch_to_block(dispatch);
    lower_enter_operation(builder, frame, entered_operations);
    store_accounting(builder, frame, accounting);
    store_control(builder, frame, control);
    return_exit(builder, Mips4BlockExit::Dispatch);
    builder.switch_to_block(delay);
    let Some(delay_slot) = block.delay_slot() else {
        lower_enter_operation(builder, frame, entered_operations);
        store_accounting(builder, frame, accounting);
        store_control(builder, frame, control);
        return_exit(builder, Mips4BlockExit::Dispatch);
        return;
    };
    entered_operations += 1;
    if operation_requires_early_count(delay_slot.operation) {
        lower_enter_operation(builder, frame, entered_operations);
        store_control(builder, frame, control);
    }
    let runtime_operation = matches!(delay_slot.operation, Mips4BlockOperation::Runtime(_));
    match lower_operation(
        builder,
        frame,
        delay_slot.operation,
        &mut runtime_index,
        entered_operations,
        &mut accounting,
        runtime_abi,
    ) {
        LoweredOperation::NeedsRetirement => {
            retire_control_sequential(builder, &mut control);
            lower_budget_check_with_control(
                builder,
                frame,
                entered_operations,
                control,
                &mut accounting,
            );
        }
        LoweredOperation::Retired => {
            if runtime_operation {
                control = load_control(builder, frame);
            }
        }
        LoweredOperation::Terminated => return,
    }
    lower_enter_operation(builder, frame, entered_operations);
    store_accounting(builder, frame, accounting);
    store_control(builder, frame, control);
    return_exit(builder, Mips4BlockExit::Dispatch);
}

const fn operation_requires_early_count(operation: Mips4BlockOperation) -> bool {
    matches!(
        operation,
        Mips4BlockOperation::Arithmetic {
            trap_on_overflow: true,
            ..
        } | Mips4BlockOperation::Trap { .. }
            | Mips4BlockOperation::Exception(_)
            | Mips4BlockOperation::Runtime(_)
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LoweredOperation {
    NeedsRetirement,
    Retired,
    Terminated,
}

#[derive(Clone, Copy)]
struct NativeRuntimeAbi {
    pointer_type: cranelift_codegen::ir::Type,
    call_conv: CallConv,
}

fn lower_operation(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    operation: Mips4BlockOperation,
    runtime_index: &mut u32,
    entered_operations: u64,
    accounting: &mut NativeAccounting,
    runtime_abi: NativeRuntimeAbi,
) -> LoweredOperation {
    match operation {
        Mips4BlockOperation::Arithmetic {
            operation,
            width,
            trap_on_overflow,
            noop_on_invalid_word,
            destination,
            lhs,
            rhs,
        } => lower_arithmetic(
            builder,
            frame,
            operation,
            width,
            trap_on_overflow,
            noop_on_invalid_word,
            destination,
            lhs,
            rhs,
            *accounting,
        ),
        Mips4BlockOperation::Logical {
            operation,
            destination,
            lhs,
            rhs,
        } => {
            let lhs = load_gpr(builder, frame, lhs);
            let rhs = lower_operand(builder, frame, rhs);
            let result = match operation {
                Mips4BlockLogical::And => builder.ins().band(lhs, rhs),
                Mips4BlockLogical::Or => builder.ins().bor(lhs, rhs),
                Mips4BlockLogical::Xor => builder.ins().bxor(lhs, rhs),
                Mips4BlockLogical::Nor => {
                    let or = builder.ins().bor(lhs, rhs);
                    builder.ins().bnot(or)
                }
            };
            store_gpr(builder, frame, destination, result);
        }
        Mips4BlockOperation::LoadUpperImmediate {
            destination,
            immediate,
        } => {
            let word = builder
                .ins()
                .iconst(types::I32, i64::from((u32::from(immediate)) << 16));
            let value = builder.ins().sextend(types::I64, word);
            store_gpr(builder, frame, destination, value);
        }
        Mips4BlockOperation::Shift {
            operation,
            width,
            noop_on_invalid_word,
            destination,
            value,
            amount,
        } => lower_shift(
            builder,
            frame,
            operation,
            width,
            noop_on_invalid_word,
            destination,
            value,
            amount,
        ),
        Mips4BlockOperation::Compare {
            comparison,
            destination,
            lhs,
            rhs,
        } => {
            let lhs = load_gpr(builder, frame, lhs);
            let rhs = lower_operand(builder, frame, rhs);
            let condition = match comparison {
                Mips4BlockComparison::SignedLessThan => IntCC::SignedLessThan,
                Mips4BlockComparison::UnsignedLessThan => IntCC::UnsignedLessThan,
            };
            let result = builder.ins().icmp(condition, lhs, rhs);
            let result = builder.ins().uextend(types::I64, result);
            store_gpr(builder, frame, destination, result);
        }
        Mips4BlockOperation::Multiply {
            width,
            signed,
            noop_on_invalid_word,
            lhs,
            rhs,
        } => lower_multiply(
            builder,
            frame,
            width,
            signed,
            noop_on_invalid_word,
            lhs,
            rhs,
        ),
        Mips4BlockOperation::Divide {
            width,
            signed,
            noop_on_invalid_word,
            lhs,
            rhs,
        } => lower_divide(
            builder,
            frame,
            width,
            signed,
            noop_on_invalid_word,
            lhs,
            rhs,
        ),
        Mips4BlockOperation::MoveFromSpecial { high, destination } => {
            let offset = if high {
                MIPS4_BLOCK_FRAME_HI_OFFSET
            } else {
                MIPS4_BLOCK_FRAME_LO_OFFSET
            };
            let value = load_i64(builder, frame, offset);
            store_gpr(builder, frame, destination, value);
        }
        Mips4BlockOperation::MoveToSpecial { high, source } => {
            let offset = if high {
                MIPS4_BLOCK_FRAME_HI_OFFSET
            } else {
                MIPS4_BLOCK_FRAME_LO_OFFSET
            };
            let value = load_gpr(builder, frame, source);
            store_i64(builder, frame, offset, value);
        }
        Mips4BlockOperation::ConditionalMove {
            when_zero,
            destination,
            source,
            condition,
        } => {
            if destination != 0 {
                let condition = load_gpr(builder, frame, condition);
                let condition = builder.ins().icmp_imm(
                    if when_zero {
                        IntCC::Equal
                    } else {
                        IntCC::NotEqual
                    },
                    condition,
                    0,
                );
                let source = load_gpr(builder, frame, source);
                let old = load_gpr(builder, frame, destination);
                let value = builder.ins().select(condition, source, old);
                store_gpr(builder, frame, destination, value);
            }
        }
        Mips4BlockOperation::Trap { trap, lhs, rhs } => {
            let lhs = load_gpr(builder, frame, lhs);
            let rhs = lower_operand(builder, frame, rhs);
            let condition = match trap {
                Mips4BlockTrap::Equal => IntCC::Equal,
                Mips4BlockTrap::NotEqual => IntCC::NotEqual,
                Mips4BlockTrap::SignedGreaterThanOrEqual => IntCC::SignedGreaterThanOrEqual,
                Mips4BlockTrap::UnsignedGreaterThanOrEqual => IntCC::UnsignedGreaterThanOrEqual,
                Mips4BlockTrap::SignedLessThan => IntCC::SignedLessThan,
                Mips4BlockTrap::UnsignedLessThan => IntCC::UnsignedLessThan,
            };
            let condition = builder.ins().icmp(condition, lhs, rhs);
            exception_if(
                builder,
                frame,
                condition,
                Mips4BlockException::Trap,
                *accounting,
            );
        }
        Mips4BlockOperation::Exception(exception) => {
            store_exception(builder, frame, exception);
            store_accounting(builder, frame, *accounting);
            return_exit(builder, Mips4BlockExit::Exception);
            return LoweredOperation::Terminated;
        }
        Mips4BlockOperation::Runtime(operation) => {
            let index = *runtime_index;
            *runtime_index = runtime_index.saturating_add(1);
            let runtime_calls = builder.ins().iconst(types::I64, i64::from(index) + 1);
            store_i64(
                builder,
                frame,
                MIPS4_BLOCK_FRAME_RUNTIME_CALLS_OFFSET,
                runtime_calls,
            );
            lower_runtime_operation(
                builder,
                frame,
                index,
                operation,
                entered_operations,
                accounting,
                runtime_abi,
            );
            return LoweredOperation::Retired;
        }
        Mips4BlockOperation::NoOperation => {}
    }
    LoweredOperation::NeedsRetirement
}

fn lower_runtime_operation(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    operation: u32,
    runtime_operation: Mips4RuntimeOperation,
    entered_operations: u64,
    accounting: &mut NativeAccounting,
    runtime_abi: NativeRuntimeAbi,
) {
    if !accounting.frame_synchronized {
        store_accounting(builder, frame, *accounting);
        accounting.frame_synchronized = true;
    }
    let context = builder.ins().load(
        runtime_abi.pointer_type,
        MemFlagsData::trusted(),
        frame,
        MIPS4_BLOCK_FRAME_RUNTIME_CONTEXT_OFFSET,
    );
    let runtime_call = builder.ins().load(
        runtime_abi.pointer_type,
        MemFlagsData::trusted(),
        frame,
        MIPS4_BLOCK_FRAME_RUNTIME_CALL_OFFSET,
    );
    let operation = builder.ins().iconst(types::I32, i64::from(operation));
    let mut signature = Signature::new(runtime_abi.call_conv);
    signature.params.extend([
        AbiParam::new(runtime_abi.pointer_type),
        AbiParam::new(runtime_abi.pointer_type),
        AbiParam::new(types::I32),
    ]);
    signature.returns.push(AbiParam::new(types::I32));
    let signature = builder.import_signature(signature);
    let call = builder
        .ins()
        .call_indirect(signature, runtime_call, &[context, frame, operation]);
    let result = builder.inst_results(call)[0];
    let continued_accounting = retire_accounting(builder, *accounting);

    let expected_result = expected_runtime_result(runtime_operation);
    let expected = builder
        .ins()
        .icmp_imm(IntCC::Equal, result, expected_result as i64);
    let expected_block = builder.create_block();
    let uncommon_block = builder.create_block();
    builder
        .ins()
        .brif(expected, expected_block, &[], uncommon_block, &[]);
    builder.switch_to_block(expected_block);
    match expected_result {
        Mips4RuntimeResult::ContinueControl => {
            lower_budget_check(builder, frame, entered_operations, continued_accounting);
        }
        Mips4RuntimeResult::DispatchControl => {
            lower_budget_check(builder, frame, entered_operations, continued_accounting);
            store_accounting(builder, frame, continued_accounting);
            return_exit(builder, Mips4BlockExit::Dispatch);
        }
        Mips4RuntimeResult::Exception => {
            return_exit(builder, Mips4BlockExit::Exception);
        }
        _ => unreachable!("runtime fast paths use only typed common outcomes"),
    }

    let continue_sequential = builder.create_block();
    let continue_control = builder.create_block();
    let dispatch_sequential = builder.create_block();
    let dispatch_control = builder.create_block();
    let transaction = builder.create_block();
    let exception = builder.create_block();
    let idle = builder.create_block();
    let invalid = builder.create_block();
    let done = builder.create_block();
    if expected_result == Mips4RuntimeResult::ContinueControl {
        builder.ins().jump(done, &[]);
    }
    builder.switch_to_block(uncommon_block);
    let rare_block = if let Some(secondary_result) = secondary_runtime_result(runtime_operation) {
        let secondary = builder
            .ins()
            .icmp_imm(IntCC::Equal, result, secondary_result as i64);
        let secondary_block = builder.create_block();
        let rare_block = builder.create_block();
        builder
            .ins()
            .brif(secondary, secondary_block, &[], rare_block, &[]);
        builder.switch_to_block(secondary_block);
        match secondary_result {
            Mips4RuntimeResult::Transaction => {
                return_exit(builder, Mips4BlockExit::RuntimeTransaction);
            }
            _ => unreachable!("runtime secondary paths use only typed common outcomes"),
        }
        Some(rare_block)
    } else {
        None
    };
    if let Some(rare_block) = rare_block {
        builder.switch_to_block(rare_block);
    }
    let mut switch = Switch::new();
    switch.set_entry(Mips4RuntimeResult::Continue as u128, continue_sequential);
    switch.set_entry(
        Mips4RuntimeResult::ContinueControl as u128,
        continue_control,
    );
    switch.set_entry(
        Mips4RuntimeResult::DispatchSequential as u128,
        dispatch_sequential,
    );
    switch.set_entry(
        Mips4RuntimeResult::DispatchControl as u128,
        dispatch_control,
    );
    switch.set_entry(Mips4RuntimeResult::Transaction as u128, transaction);
    switch.set_entry(Mips4RuntimeResult::Exception as u128, exception);
    switch.set_entry(Mips4RuntimeResult::Idle as u128, idle);
    switch.set_entry(Mips4RuntimeResult::InternalError as u128, invalid);
    switch.emit(builder, result, invalid);

    builder.switch_to_block(continue_sequential);
    lower_retire_sequential(builder, frame);
    lower_budget_check(builder, frame, entered_operations, continued_accounting);
    builder.ins().jump(done, &[]);

    builder.switch_to_block(continue_control);
    lower_budget_check(builder, frame, entered_operations, continued_accounting);
    builder.ins().jump(done, &[]);

    builder.switch_to_block(dispatch_sequential);
    lower_retire_sequential(builder, frame);
    lower_budget_check(builder, frame, entered_operations, continued_accounting);
    store_accounting(builder, frame, continued_accounting);
    return_exit(builder, Mips4BlockExit::Dispatch);

    builder.switch_to_block(dispatch_control);
    lower_budget_check(builder, frame, entered_operations, continued_accounting);
    store_accounting(builder, frame, continued_accounting);
    return_exit(builder, Mips4BlockExit::Dispatch);

    builder.switch_to_block(transaction);
    return_exit(builder, Mips4BlockExit::RuntimeTransaction);

    builder.switch_to_block(exception);
    return_exit(builder, Mips4BlockExit::Exception);

    builder.switch_to_block(idle);
    store_accounting(builder, frame, continued_accounting);
    return_exit(builder, Mips4BlockExit::RuntimeIdle);

    builder.switch_to_block(invalid);
    return_exit(builder, Mips4BlockExit::InternalError);

    builder.switch_to_block(done);
    *accounting = continued_accounting;
}

const fn expected_runtime_result(operation: Mips4RuntimeOperation) -> Mips4RuntimeResult {
    operation.synchronous_result()
}

const fn secondary_runtime_result(operation: Mips4RuntimeOperation) -> Option<Mips4RuntimeResult> {
    match operation {
        Mips4RuntimeOperation::Memory { .. }
        | Mips4RuntimeOperation::Prefetch { .. }
        | Mips4RuntimeOperation::Cp1 { .. }
        | Mips4RuntimeOperation::Cache { .. } => Some(Mips4RuntimeResult::Transaction),
        Mips4RuntimeOperation::Cp0 { .. }
        | Mips4RuntimeOperation::Coprocessor { .. }
        | Mips4RuntimeOperation::Raise(_) => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn lower_arithmetic(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    operation: Mips4BlockArithmetic,
    width: Mips4BlockWidth,
    trap_on_overflow: bool,
    noop_on_invalid_word: bool,
    destination: u8,
    lhs_register: u8,
    rhs_operand: Mips4BlockOperand,
    accounting: NativeAccounting,
) {
    let lhs = load_gpr(builder, frame, lhs_register);
    let rhs = lower_operand(builder, frame, rhs_operand);
    let valid = if width == Mips4BlockWidth::Word && noop_on_invalid_word {
        let mut valid = word_valid(builder, lhs);
        if matches!(rhs_operand, Mips4BlockOperand::Register(_)) {
            let rhs_valid = word_valid(builder, rhs);
            valid = builder.ins().band(valid, rhs_valid);
        }
        Some(valid)
    } else {
        None
    };

    let (result, overflow) = match width {
        Mips4BlockWidth::Word => {
            let lhs = builder.ins().ireduce(types::I32, lhs);
            let rhs = builder.ins().ireduce(types::I32, rhs);
            let result = match operation {
                Mips4BlockArithmetic::Add => builder.ins().iadd(lhs, rhs),
                Mips4BlockArithmetic::Subtract => builder.ins().isub(lhs, rhs),
            };
            let overflow_bits = match operation {
                Mips4BlockArithmetic::Add => {
                    let lhs_result = builder.ins().bxor(lhs, result);
                    let rhs_result = builder.ins().bxor(rhs, result);
                    builder.ins().band(lhs_result, rhs_result)
                }
                Mips4BlockArithmetic::Subtract => {
                    let lhs_rhs = builder.ins().bxor(lhs, rhs);
                    let lhs_result = builder.ins().bxor(lhs, result);
                    builder.ins().band(lhs_rhs, lhs_result)
                }
            };
            let overflow = builder
                .ins()
                .icmp_imm(IntCC::SignedLessThan, overflow_bits, 0);
            (builder.ins().sextend(types::I64, result), overflow)
        }
        Mips4BlockWidth::Doubleword => {
            let result = match operation {
                Mips4BlockArithmetic::Add => builder.ins().iadd(lhs, rhs),
                Mips4BlockArithmetic::Subtract => builder.ins().isub(lhs, rhs),
            };
            let overflow_bits = match operation {
                Mips4BlockArithmetic::Add => {
                    let lhs_result = builder.ins().bxor(lhs, result);
                    let rhs_result = builder.ins().bxor(rhs, result);
                    builder.ins().band(lhs_result, rhs_result)
                }
                Mips4BlockArithmetic::Subtract => {
                    let lhs_rhs = builder.ins().bxor(lhs, rhs);
                    let lhs_result = builder.ins().bxor(lhs, result);
                    builder.ins().band(lhs_rhs, lhs_result)
                }
            };
            let overflow = builder
                .ins()
                .icmp_imm(IntCC::SignedLessThan, overflow_bits, 0);
            (result, overflow)
        }
    };
    if trap_on_overflow {
        let condition = if let Some(valid) = valid {
            builder.ins().band(valid, overflow)
        } else {
            overflow
        };
        exception_if(
            builder,
            frame,
            condition,
            Mips4BlockException::ArithmeticOverflow,
            accounting,
        );
    }
    let result = if let Some(valid) = valid {
        let old = load_gpr(builder, frame, destination);
        builder.ins().select(valid, result, old)
    } else {
        result
    };
    store_gpr(builder, frame, destination, result);
}

#[allow(clippy::too_many_arguments)]
fn lower_shift(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    operation: Mips4BlockShift,
    width: Mips4BlockWidth,
    noop_on_invalid_word: bool,
    destination: u8,
    value_register: u8,
    amount: Mips4BlockShiftAmount,
) {
    let value = load_gpr(builder, frame, value_register);
    let valid = (width == Mips4BlockWidth::Word && noop_on_invalid_word)
        .then(|| word_valid(builder, value));
    let result = match width {
        Mips4BlockWidth::Word => {
            let value = builder.ins().ireduce(types::I32, value);
            let amount = lower_shift_amount(builder, frame, amount, types::I32, 31);
            let result = match operation {
                Mips4BlockShift::Left => builder.ins().ishl(value, amount),
                Mips4BlockShift::RightLogical => builder.ins().ushr(value, amount),
                Mips4BlockShift::RightArithmetic => builder.ins().sshr(value, amount),
            };
            builder.ins().sextend(types::I64, result)
        }
        Mips4BlockWidth::Doubleword => {
            let amount = lower_shift_amount(builder, frame, amount, types::I64, 63);
            match operation {
                Mips4BlockShift::Left => builder.ins().ishl(value, amount),
                Mips4BlockShift::RightLogical => builder.ins().ushr(value, amount),
                Mips4BlockShift::RightArithmetic => builder.ins().sshr(value, amount),
            }
        }
    };
    let result = if let Some(valid) = valid {
        let old = load_gpr(builder, frame, destination);
        builder.ins().select(valid, result, old)
    } else {
        result
    };
    store_gpr(builder, frame, destination, result);
}

fn lower_multiply(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    width: Mips4BlockWidth,
    signed: bool,
    noop_on_invalid_word: bool,
    lhs: u8,
    rhs: u8,
) {
    let lhs = load_gpr(builder, frame, lhs);
    let rhs = load_gpr(builder, frame, rhs);
    let valid = if width == Mips4BlockWidth::Word && noop_on_invalid_word {
        let lhs_valid = word_valid(builder, lhs);
        let rhs_valid = word_valid(builder, rhs);
        Some(builder.ins().band(lhs_valid, rhs_valid))
    } else {
        None
    };
    let (hi, lo) = match width {
        Mips4BlockWidth::Word => {
            let lhs = builder.ins().ireduce(types::I32, lhs);
            let rhs = builder.ins().ireduce(types::I32, rhs);
            let lhs = if signed {
                builder.ins().sextend(types::I64, lhs)
            } else {
                builder.ins().uextend(types::I64, lhs)
            };
            let rhs = if signed {
                builder.ins().sextend(types::I64, rhs)
            } else {
                builder.ins().uextend(types::I64, rhs)
            };
            let product = builder.ins().imul(lhs, rhs);
            let lo = builder.ins().ireduce(types::I32, product);
            let high = builder.ins().sshr_imm(product, 32);
            let hi = builder.ins().ireduce(types::I32, high);
            (
                builder.ins().sextend(types::I64, hi),
                builder.ins().sextend(types::I64, lo),
            )
        }
        Mips4BlockWidth::Doubleword => {
            let lo = builder.ins().imul(lhs, rhs);
            let hi = if signed {
                builder.ins().smulhi(lhs, rhs)
            } else {
                builder.ins().umulhi(lhs, rhs)
            };
            (hi, lo)
        }
    };
    let hi = select_old_if_invalid(builder, frame, MIPS4_BLOCK_FRAME_HI_OFFSET, valid, hi);
    let lo = select_old_if_invalid(builder, frame, MIPS4_BLOCK_FRAME_LO_OFFSET, valid, lo);
    store_i64(builder, frame, MIPS4_BLOCK_FRAME_HI_OFFSET, hi);
    store_i64(builder, frame, MIPS4_BLOCK_FRAME_LO_OFFSET, lo);
}

fn lower_divide(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    width: Mips4BlockWidth,
    signed: bool,
    noop_on_invalid_word: bool,
    lhs: u8,
    rhs: u8,
) {
    let lhs64 = load_gpr(builder, frame, lhs);
    let rhs64 = load_gpr(builder, frame, rhs);
    let (lhs, rhs, integer_type) = match width {
        Mips4BlockWidth::Word => (
            builder.ins().ireduce(types::I32, lhs64),
            builder.ins().ireduce(types::I32, rhs64),
            types::I32,
        ),
        Mips4BlockWidth::Doubleword => (lhs64, rhs64, types::I64),
    };
    let zero = builder.ins().icmp_imm(IntCC::Equal, rhs, 0);
    let signed_overflow = if signed {
        let minimum = if integer_type == types::I32 {
            i64::from(i32::MIN)
        } else {
            i64::MIN
        };
        let lhs_minimum = builder.ins().icmp_imm(IntCC::Equal, lhs, minimum);
        let rhs_negative_one = builder.ins().icmp_imm(IntCC::Equal, rhs, -1);
        builder.ins().band(lhs_minimum, rhs_negative_one)
    } else {
        builder.ins().iconst(types::I8, 0)
    };
    let undefined = builder.ins().bor(zero, signed_overflow);
    let mut allowed = builder.ins().icmp_imm(IntCC::Equal, undefined, 0);
    if width == Mips4BlockWidth::Word && noop_on_invalid_word {
        let lhs_valid = word_valid(builder, lhs64);
        let rhs_valid = word_valid(builder, rhs64);
        let valid = builder.ins().band(lhs_valid, rhs_valid);
        allowed = builder.ins().band(allowed, valid);
    }

    let execute = builder.create_block();
    let done = builder.create_block();
    builder.ins().brif(allowed, execute, &[], done, &[]);
    builder.switch_to_block(execute);
    let quotient = if signed {
        builder.ins().sdiv(lhs, rhs)
    } else {
        builder.ins().udiv(lhs, rhs)
    };
    let remainder = if signed {
        builder.ins().srem(lhs, rhs)
    } else {
        builder.ins().urem(lhs, rhs)
    };
    let (hi, lo) = if integer_type == types::I32 {
        (
            builder.ins().sextend(types::I64, remainder),
            builder.ins().sextend(types::I64, quotient),
        )
    } else {
        (remainder, quotient)
    };
    store_i64(builder, frame, MIPS4_BLOCK_FRAME_HI_OFFSET, hi);
    store_i64(builder, frame, MIPS4_BLOCK_FRAME_LO_OFFSET, lo);
    builder.ins().jump(done, &[]);
    builder.switch_to_block(done);
}

fn lower_retire_sequential(builder: &mut FunctionBuilder<'_>, frame: Value) {
    let next_pc = load_i64(builder, frame, MIPS4_BLOCK_FRAME_NEXT_PC_OFFSET);
    let following = builder.ins().iadd_imm(next_pc, 4);
    let zero = builder.ins().iconst(types::I64, 0);
    store_i64(builder, frame, MIPS4_BLOCK_FRAME_PC_OFFSET, next_pc);
    store_i64(builder, frame, MIPS4_BLOCK_FRAME_NEXT_PC_OFFSET, following);
    store_i64(builder, frame, MIPS4_BLOCK_FRAME_DELAY_PC_OFFSET, zero);
    store_i64(builder, frame, MIPS4_BLOCK_FRAME_DELAY_VALID_OFFSET, zero);
}

#[derive(Clone, Copy)]
struct NativeControl {
    pc: Value,
    next_pc: Value,
    delay_pc: Value,
    delay_valid: Value,
}

#[derive(Clone, Copy)]
struct NativeAccounting {
    budget: Value,
    retired: Value,
    frame_synchronized: bool,
}

fn load_control(builder: &mut FunctionBuilder<'_>, frame: Value) -> NativeControl {
    NativeControl {
        pc: load_i64(builder, frame, MIPS4_BLOCK_FRAME_PC_OFFSET),
        next_pc: load_i64(builder, frame, MIPS4_BLOCK_FRAME_NEXT_PC_OFFSET),
        delay_pc: load_i64(builder, frame, MIPS4_BLOCK_FRAME_DELAY_PC_OFFSET),
        delay_valid: load_i64(builder, frame, MIPS4_BLOCK_FRAME_DELAY_VALID_OFFSET),
    }
}

fn load_accounting(builder: &mut FunctionBuilder<'_>, frame: Value) -> NativeAccounting {
    NativeAccounting {
        budget: load_i64(builder, frame, MIPS4_BLOCK_FRAME_BUDGET_OFFSET),
        retired: load_i64(builder, frame, MIPS4_BLOCK_FRAME_RETIRED_OFFSET),
        frame_synchronized: true,
    }
}

fn store_control(builder: &mut FunctionBuilder<'_>, frame: Value, control: NativeControl) {
    store_i64(builder, frame, MIPS4_BLOCK_FRAME_PC_OFFSET, control.pc);
    store_i64(
        builder,
        frame,
        MIPS4_BLOCK_FRAME_NEXT_PC_OFFSET,
        control.next_pc,
    );
    store_i64(
        builder,
        frame,
        MIPS4_BLOCK_FRAME_DELAY_PC_OFFSET,
        control.delay_pc,
    );
    store_i64(
        builder,
        frame,
        MIPS4_BLOCK_FRAME_DELAY_VALID_OFFSET,
        control.delay_valid,
    );
}

fn store_accounting(builder: &mut FunctionBuilder<'_>, frame: Value, accounting: NativeAccounting) {
    store_i64(
        builder,
        frame,
        MIPS4_BLOCK_FRAME_BUDGET_OFFSET,
        accounting.budget,
    );
    store_i64(
        builder,
        frame,
        MIPS4_BLOCK_FRAME_RETIRED_OFFSET,
        accounting.retired,
    );
}

fn retire_control_sequential(builder: &mut FunctionBuilder<'_>, control: &mut NativeControl) {
    control.pc = control.next_pc;
    control.next_pc = builder.ins().iadd_imm(control.next_pc, 4);
    control.delay_pc = builder.ins().iconst(types::I64, 0);
    control.delay_valid = builder.ins().iconst(types::I64, 0);
}

fn lower_enter_operation(builder: &mut FunctionBuilder<'_>, frame: Value, entered_operations: u64) {
    let entered = builder.ins().iconst(types::I64, entered_operations as i64);
    store_i64(
        builder,
        frame,
        MIPS4_BLOCK_FRAME_OPERATIONS_EXECUTED_OFFSET,
        entered,
    );
}

fn retire_branch_control(
    builder: &mut FunctionBuilder<'_>,
    control: &mut NativeControl,
    branch_pc: u64,
    target: Value,
    taken: Value,
    nullify: Value,
) {
    let old_next = control.next_pc;
    let fallthrough = builder.ins().iadd_imm(old_next, 4);
    let after_fallthrough = builder.ins().iadd_imm(old_next, 8);
    let delayed_next = builder.ins().select(taken, target, fallthrough);
    let pc = builder.ins().select(nullify, fallthrough, old_next);
    let next_pc = builder
        .ins()
        .select(nullify, after_fallthrough, delayed_next);
    let branch_pc = builder.ins().iconst(types::I64, branch_pc as i64);
    let zero = builder.ins().iconst(types::I64, 0);
    let one = builder.ins().iconst(types::I64, 1);
    let delay_pc = builder.ins().select(nullify, zero, branch_pc);
    let delay_valid = builder.ins().select(nullify, zero, one);
    control.pc = pc;
    control.next_pc = next_pc;
    control.delay_pc = delay_pc;
    control.delay_valid = delay_valid;
}

fn lower_budget_check_with_control(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    entered_operations: u64,
    control: NativeControl,
    accounting: &mut NativeAccounting,
) {
    *accounting = retire_accounting(builder, *accounting);
    let exhausted = builder.ins().icmp_imm(IntCC::Equal, accounting.budget, 0);
    let exit = builder.create_block();
    let continue_block = builder.create_block();
    builder
        .ins()
        .brif(exhausted, exit, &[], continue_block, &[]);
    builder.switch_to_block(exit);
    lower_enter_operation(builder, frame, entered_operations);
    store_accounting(builder, frame, *accounting);
    store_control(builder, frame, control);
    return_exit(builder, Mips4BlockExit::BudgetExhausted);
    builder.switch_to_block(continue_block);
}

fn lower_budget_check(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    entered_operations: u64,
    accounting: NativeAccounting,
) {
    let exhausted = builder.ins().icmp_imm(IntCC::Equal, accounting.budget, 0);
    let exit = builder.create_block();
    let continue_block = builder.create_block();
    builder
        .ins()
        .brif(exhausted, exit, &[], continue_block, &[]);
    builder.switch_to_block(exit);
    lower_enter_operation(builder, frame, entered_operations);
    store_accounting(builder, frame, accounting);
    return_exit(builder, Mips4BlockExit::BudgetExhausted);
    builder.switch_to_block(continue_block);
}

fn retire_accounting(
    builder: &mut FunctionBuilder<'_>,
    accounting: NativeAccounting,
) -> NativeAccounting {
    NativeAccounting {
        budget: builder.ins().iadd_imm(accounting.budget, -1),
        retired: builder.ins().iadd_imm(accounting.retired, 1),
        frame_synchronized: false,
    }
}

fn lower_branch_condition(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    condition: Mips4BlockBranchCondition,
) -> Value {
    match condition {
        Mips4BlockBranchCondition::Always => builder.ins().iconst(types::I8, 1),
        Mips4BlockBranchCondition::Equal { lhs, rhs } => {
            let lhs = load_gpr(builder, frame, lhs);
            let rhs = load_gpr(builder, frame, rhs);
            builder.ins().icmp(IntCC::Equal, lhs, rhs)
        }
        Mips4BlockBranchCondition::NotEqual { lhs, rhs } => {
            let lhs = load_gpr(builder, frame, lhs);
            let rhs = load_gpr(builder, frame, rhs);
            builder.ins().icmp(IntCC::NotEqual, lhs, rhs)
        }
        Mips4BlockBranchCondition::LessThanZero { source } => {
            let source = load_gpr(builder, frame, source);
            builder.ins().icmp_imm(IntCC::SignedLessThan, source, 0)
        }
        Mips4BlockBranchCondition::GreaterThanOrEqualZero { source } => {
            let source = load_gpr(builder, frame, source);
            builder
                .ins()
                .icmp_imm(IntCC::SignedGreaterThanOrEqual, source, 0)
        }
        Mips4BlockBranchCondition::LessThanOrEqualZero { source } => {
            let source = load_gpr(builder, frame, source);
            builder
                .ins()
                .icmp_imm(IntCC::SignedLessThanOrEqual, source, 0)
        }
        Mips4BlockBranchCondition::GreaterThanZero { source } => {
            let source = load_gpr(builder, frame, source);
            builder.ins().icmp_imm(IntCC::SignedGreaterThan, source, 0)
        }
    }
}

fn lower_shift_amount(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    amount: Mips4BlockShiftAmount,
    value_type: cranelift_codegen::ir::Type,
    mask: i64,
) -> Value {
    let amount = match amount {
        Mips4BlockShiftAmount::Immediate(amount) => {
            builder.ins().iconst(value_type, i64::from(amount))
        }
        Mips4BlockShiftAmount::Register(register) => {
            let amount = load_gpr(builder, frame, register);
            if value_type == types::I32 {
                builder.ins().ireduce(types::I32, amount)
            } else {
                amount
            }
        }
    };
    builder.ins().band_imm(amount, mask)
}

fn lower_operand(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    operand: Mips4BlockOperand,
) -> Value {
    match operand {
        Mips4BlockOperand::Register(register) => load_gpr(builder, frame, register),
        Mips4BlockOperand::SignedImmediate(immediate) => {
            builder.ins().iconst(types::I64, i64::from(immediate))
        }
        Mips4BlockOperand::UnsignedImmediate(immediate) => {
            builder.ins().iconst(types::I64, i64::from(immediate))
        }
    }
}

fn word_valid(builder: &mut FunctionBuilder<'_>, value: Value) -> Value {
    let word = builder.ins().ireduce(types::I32, value);
    let extended = builder.ins().sextend(types::I64, word);
    builder.ins().icmp(IntCC::Equal, value, extended)
}

fn select_old_if_invalid(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    offset: i32,
    valid: Option<Value>,
    value: Value,
) -> Value {
    if let Some(valid) = valid {
        let old = load_i64(builder, frame, offset);
        builder.ins().select(valid, value, old)
    } else {
        value
    }
}

fn exception_if(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    condition: Value,
    exception: Mips4BlockException,
    accounting: NativeAccounting,
) {
    let exception_block = builder.create_block();
    let continue_block = builder.create_block();
    builder
        .ins()
        .brif(condition, exception_block, &[], continue_block, &[]);
    builder.switch_to_block(exception_block);
    store_exception(builder, frame, exception);
    store_accounting(builder, frame, accounting);
    return_exit(builder, Mips4BlockExit::Exception);
    builder.switch_to_block(continue_block);
}

fn store_exception(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    exception: Mips4BlockException,
) {
    let exception = builder.ins().iconst(types::I64, exception as i64);
    store_i64(
        builder,
        frame,
        MIPS4_BLOCK_FRAME_EXCEPTION_OFFSET,
        exception,
    );
}

fn return_exit(builder: &mut FunctionBuilder<'_>, exit: Mips4BlockExit) {
    let exit = builder.ins().iconst(types::I32, exit as i64);
    builder.ins().return_(&[exit]);
}

fn load_gpr(builder: &mut FunctionBuilder<'_>, frame: Value, register: u8) -> Value {
    if register == 0 {
        builder.ins().iconst(types::I64, 0)
    } else {
        load_i64(builder, frame, mips4_block_frame_gpr_offset(register))
    }
}

fn store_gpr(builder: &mut FunctionBuilder<'_>, frame: Value, register: u8, value: Value) {
    if register != 0 {
        store_i64(
            builder,
            frame,
            mips4_block_frame_gpr_offset(register),
            value,
        );
        let pointer_type = builder.func.dfg.value_type(frame);
        let write_through = builder.ins().load(
            pointer_type,
            MemFlagsData::trusted(),
            frame,
            MIPS4_BLOCK_FRAME_GPR_WRITE_THROUGH_OFFSET,
        );
        store_i64(builder, write_through, i32::from(register) * 8, value);
    }
}

fn load_i64(builder: &mut FunctionBuilder<'_>, frame: Value, offset: i32) -> Value {
    builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), frame, offset)
}

fn store_i64(builder: &mut FunctionBuilder<'_>, frame: Value, offset: i32, value: Value) {
    builder
        .ins()
        .store(MemFlagsData::trusted(), value, frame, offset);
}

#[cfg(test)]
mod tests {
    use se_device::cpu::mips4::config::{Mips4CacheConfig, Mips4Endianness};
    use se_device::cpu::mips4::execution::block::{
        Mips4BlockBranch, Mips4BlockEngine, Mips4BlockGuard, Mips4BlockInstruction,
        Mips4BlockInstructionMetadata, Mips4BlockKey, Mips4BlockLiftedInstruction,
        Mips4BlockRetire, Mips4BlockRuntime, Mips4BlockTier, Mips4RuntimeOperation,
        interpret_block, lift_cpu_instruction,
    };
    use se_device::cpu::mips4::instruction::Mips4Instruction;
    use se_device::cpu::mips4::instruction::decode::{
        Mips4InstructionClass, Mips4InstructionDecode, decode_instruction,
    };
    use se_device::cpu::mips4::model::r5000::boot_mode::R5000BootMode;
    use se_device::cpu::mips4::model::r5000::execution_policy::R5000ExecutionPolicy;
    use se_device::cpu::mips4::model::r5000::profile::R5000Profile;
    use se_device::cpu::mips4::model::r5000::revision::R5000Revision;

    use super::*;

    fn policy() -> R5000ExecutionPolicy {
        R5000ExecutionPolicy::new(
            R5000Profile::new(
                Mips4Endianness::Big,
                R5000Revision::from_bits(0x21),
                180_000_000,
                Mips4CacheConfig::present(32 * 1024, 32),
                Mips4CacheConfig::present(32 * 1024, 32),
                Mips4CacheConfig::disabled(),
            ),
            R5000BootMode::from_low_bits(0).unwrap(),
        )
    }

    fn sequential(
        pc: u64,
        bits: u32,
    ) -> se_device::cpu::mips4::execution::block::Mips4BlockInstruction {
        sequential_with_delay(pc, bits, None)
    }

    fn sequential_with_delay(
        pc: u64,
        bits: u32,
        delay_slot_branch_pc: Option<u64>,
    ) -> se_device::cpu::mips4::execution::block::Mips4BlockInstruction {
        let raw = Mips4Instruction::from_bits(bits);
        let Mips4InstructionDecode::Instruction(Mips4InstructionClass::Cpu(decoded)) =
            decode_instruction(raw)
        else {
            panic!()
        };
        let Mips4BlockLiftedInstruction::Sequential(instruction) = lift_cpu_instruction(
            &policy(),
            Mips4BlockInstructionMetadata {
                pc,
                instruction: bits,
                delay_slot_branch_pc,
            },
            decoded,
        ) else {
            panic!()
        };
        instruction
    }

    fn branch(pc: u64, bits: u32) -> Mips4BlockBranch {
        let raw = Mips4Instruction::from_bits(bits);
        let Mips4InstructionDecode::Instruction(Mips4InstructionClass::Cpu(decoded)) =
            decode_instruction(raw)
        else {
            panic!()
        };
        let Mips4BlockLiftedInstruction::Branch(branch) = lift_cpu_instruction(
            &policy(),
            Mips4BlockInstructionMetadata {
                pc,
                instruction: bits,
                delay_slot_branch_pc: None,
            },
            decoded,
        ) else {
            panic!()
        };
        branch
    }

    fn compare_native(block: &Mips4Block, frame: Mips4BlockFrame) {
        let mut interpreted = frame.clone();
        let interpreted_exit = interpret_block(block, &mut interpreted);
        let mut native = frame;
        let mut backend = CraneliftMips4Backend::new().unwrap();
        let compiled = backend.compile(block).unwrap();
        let native_exit = backend.execute(&compiled, &mut native).unwrap();
        assert_eq!(native_exit, interpreted_exit);
        assert_eq!(native, interpreted);
    }

    #[test]
    fn native_and_interpreted_integer_blocks_match() {
        let key = Mips4BlockKey {
            pc: 0x1000,
            next_pc: 0x1004,
            delay_slot_branch_pc: None,
            fetch_context: 0,
            translation_generation: 0,
            code_guard: 0,
        };
        let mut block = Mips4Block::new(key, Mips4BlockGuard::new());
        block
            .push(sequential(
                0x1000,
                (0x09_u32 << 26) | (1 << 21) | (2 << 16) | 7,
            ))
            .unwrap();
        block
            .push(sequential(
                0x1004,
                (0x0d_u32 << 26) | (2 << 21) | (3 << 16) | 0x10,
            ))
            .unwrap();
        block.terminate_dispatch().unwrap();
        let mut gpr = [0; 32];
        gpr[1] = 5;
        let mut interpreted = Mips4BlockFrame::new(gpr, 0, 0, 0x1000, 0x1004, None, 2);
        let mut native = interpreted.clone();
        assert_eq!(
            interpret_block(&block, &mut interpreted),
            Mips4BlockExit::BudgetExhausted
        );

        let mut backend = CraneliftMips4Backend::new().unwrap();
        let compiled = backend.compile(&block).unwrap();
        assert_eq!(
            backend.execute(&compiled, &mut native).unwrap(),
            Mips4BlockExit::BudgetExhausted
        );
        assert_eq!(native, interpreted);
    }

    #[test]
    fn native_branch_likely_budget_and_delay_slot_match_interpreter() {
        let key = Mips4BlockKey {
            pc: 0x1000,
            next_pc: 0x1004,
            delay_slot_branch_pc: None,
            fetch_context: 0,
            translation_generation: 0,
            code_guard: 0,
        };
        let mut block = Mips4Block::new(key, Mips4BlockGuard::new());
        let branch_bits = (0x14_u32 << 26) | (1 << 21) | (2 << 16) | 3;
        let delay_bits = (0x09_u32 << 26) | (3 << 21) | (3 << 16) | 1;
        block
            .terminate_with_branch(
                branch(0x1000, branch_bits),
                sequential_with_delay(0x1004, delay_bits, Some(0x1000)),
            )
            .unwrap();

        for (lhs, rhs, budget) in [(1, 2, 2), (7, 7, 1), (7, 7, 2)] {
            let mut gpr = [0; 32];
            gpr[1] = lhs;
            gpr[2] = rhs;
            compare_native(
                &block,
                Mips4BlockFrame::new(gpr, 0, 0, 0x1000, 0x1004, None, budget),
            );
        }
    }

    #[test]
    fn native_hilo_trap_and_overflow_paths_match_interpreter() {
        let key = Mips4BlockKey {
            pc: 0x2000,
            next_pc: 0x2004,
            delay_slot_branch_pc: None,
            fetch_context: 0,
            translation_generation: 0,
            code_guard: 0,
        };
        let mut block = Mips4Block::new(key, Mips4BlockGuard::new());
        for (index, bits) in [
            (1_u32 << 21) | (2 << 16) | 0x1c,
            (3 << 11) | 0x12,
            (4 << 21) | (5 << 16) | 0x1e,
            (6 << 11) | 0x10,
            (7 << 21) | (8 << 16) | 0x34,
        ]
        .into_iter()
        .enumerate()
        {
            block
                .push(sequential(0x2000 + index as u64 * 4, bits))
                .unwrap();
        }
        block.terminate_dispatch().unwrap();
        let mut gpr = [0; 32];
        gpr[1] = u64::MAX - 4;
        gpr[2] = 7;
        gpr[4] = 100;
        gpr[5] = 9;
        gpr[7] = 0x55;
        gpr[8] = 0x55;
        compare_native(
            &block,
            Mips4BlockFrame::new(gpr, 0, 0, 0x2000, 0x2004, None, 5),
        );

        let mut overflow = Mips4Block::new(key, Mips4BlockGuard::new());
        overflow
            .push(sequential(
                0x2000,
                (1_u32 << 21) | (2 << 16) | (3 << 11) | 0x2c,
            ))
            .unwrap();
        overflow.terminate_dispatch().unwrap();
        let mut gpr = [0; 32];
        gpr[1] = i64::MAX as u64;
        gpr[2] = 1;
        gpr[3] = 0x1234;
        compare_native(
            &overflow,
            Mips4BlockFrame::new(gpr, 0, 0, 0x2000, 0x2004, None, 1),
        );
    }

    #[test]
    fn generated_integer_corpus_matches_native_execution() {
        let key = Mips4BlockKey {
            pc: 0x3000,
            next_pc: 0x3004,
            delay_slot_branch_pc: None,
            fetch_context: 0,
            translation_generation: 0,
            code_guard: 0,
        };
        let mut block = Mips4Block::new(key, Mips4BlockGuard::new());
        for (index, bits) in [
            (1_u32 << 21) | (2 << 16) | (9 << 11) | 0x21,
            (1_u32 << 21) | (2 << 16) | (10 << 11) | 0x2d,
            (1_u32 << 21) | (2 << 16) | (11 << 11) | 0x26,
            (3_u32 << 21) | (2 << 16) | (12 << 11) | 0x14,
            (1_u32 << 21) | (2 << 16) | (13 << 11) | 0x2b,
            (1_u32 << 21) | (2 << 16) | 0x18,
            (1_u32 << 21) | (2 << 16) | 0x1a,
            (14 << 11) | 0x12,
        ]
        .into_iter()
        .enumerate()
        {
            block
                .push(sequential(0x3000 + index as u64 * 4, bits))
                .unwrap();
        }
        block.terminate_dispatch().unwrap();
        let mut backend = CraneliftMips4Backend::new().unwrap();
        let compiled = backend.compile(&block).unwrap();
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        for case in 0..256 {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let mut gpr = [0; 32];
            gpr[1] = match case {
                0 => i64::MIN as u64,
                1 => 0,
                _ => seed,
            };
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            gpr[2] = match case {
                0 => u64::MAX,
                1 => 0,
                _ => seed,
            };
            gpr[3] = seed.rotate_left(17);
            let frame = Mips4BlockFrame::new(gpr, seed, !seed, 0x3000, 0x3004, None, 8);
            let mut interpreted = frame.clone();
            let interpreted_exit = interpret_block(&block, &mut interpreted);
            let mut native = frame;
            let native_exit = backend.execute(&compiled, &mut native).unwrap();
            assert_eq!(native_exit, interpreted_exit, "case {case}");
            assert_eq!(native, interpreted, "case {case}");
        }
    }

    #[test]
    fn native_runtime_calls_use_the_typed_trampoline() {
        struct Runtime {
            result: Mips4RuntimeResult,
        }

        impl Mips4BlockRuntime for Runtime {
            fn execute(
                &mut self,
                frame: &mut Mips4BlockFrame,
                operation: Mips4RuntimeOperation,
            ) -> Mips4RuntimeResult {
                assert!(matches!(operation, Mips4RuntimeOperation::Prefetch { .. }));
                frame.write_gpr(7, frame.read_gpr(7).wrapping_add(1));
                self.result
            }
        }

        let key = Mips4BlockKey {
            pc: 0x4000,
            next_pc: 0x4004,
            delay_slot_branch_pc: None,
            fetch_context: 0,
            translation_generation: 0,
            code_guard: 0,
        };
        let metadata = Mips4BlockInstructionMetadata {
            pc: key.pc,
            instruction: 0,
            delay_slot_branch_pc: None,
        };
        let mut block = Mips4Block::new(key, Mips4BlockGuard::new());
        block
            .push(Mips4BlockInstruction {
                metadata,
                operation: Mips4BlockOperation::Runtime(Mips4RuntimeOperation::Prefetch {
                    raw: Mips4Instruction::from_bits(0),
                }),
                retire: Mips4BlockRetire { pc: key.pc },
            })
            .unwrap();
        block.terminate_dispatch().unwrap();

        let mut engine = Mips4BlockEngine::new(CraneliftMips4Backend::new().unwrap());
        engine.insert(block).unwrap();
        let mut runtime = Runtime {
            result: Mips4RuntimeResult::Continue,
        };
        for entry in 0..257 {
            let mut frame = Mips4BlockFrame::new([0; 32], 0, 0, 0x4000, 0x4004, None, 1);
            let execution = engine
                .execute_with_runtime(key, &mut frame, &mut runtime)
                .unwrap();
            assert_eq!(frame.read_gpr(7), 1);
            assert_eq!(frame.runtime_calls(), 1);
            if entry == 256 {
                assert_eq!(execution.tier, Mips4BlockTier::Native);
            }
        }
        assert_eq!(engine.statistics().compiled_blocks, 1);
        assert_eq!(engine.statistics().native_operations, 1);
        assert_eq!(engine.statistics().runtime_calls, 257);

        runtime.result = Mips4RuntimeResult::Idle;
        let mut frame = Mips4BlockFrame::new([0; 32], 0, 0, 0x4000, 0x4004, None, 1);
        let execution = engine
            .execute_with_runtime(key, &mut frame, &mut runtime)
            .unwrap();
        assert_eq!(execution.tier, Mips4BlockTier::Native);
        assert_eq!(execution.exit, Mips4BlockExit::RuntimeIdle);
        assert_eq!(frame.retired(), 1);
        assert_eq!(frame.pc(), 0x4000);
    }
}
