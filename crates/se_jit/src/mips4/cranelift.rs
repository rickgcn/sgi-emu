//! Cranelift backend for typed MIPS IV basic blocks.

use core::{fmt, mem, ptr::NonNull};

use cranelift_codegen::ir::{
    AbiParam, BlockArg, Function, InstBuilder, MemFlagsData, Signature, Value, condcodes::IntCC,
    types,
};
use cranelift_codegen::isa::CallConv;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Switch};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module, default_libcall_names};
use se_device::cpu::mips4::execution::block::{
    Mips4Block, Mips4BlockArithmetic, Mips4BlockBranchCondition, Mips4BlockBranchTarget,
    Mips4BlockComparison, Mips4BlockException, Mips4BlockExit, Mips4BlockFrame, Mips4BlockLogical,
    Mips4BlockOperand, Mips4BlockOperation, Mips4BlockRuntime, Mips4BlockShift,
    Mips4BlockShiftAmount, Mips4BlockTrap, Mips4BlockWidth, Mips4FastMemoryRuntime,
    Mips4RuntimeOperation, Mips4RuntimeResult,
};
use se_device::cpu::mips4::instruction::decode::Mips4CpuInstruction;

use super::abi::*;
use super::engine::Mips4CodegenBackend;
use super::region::{Mips4Region, Mips4RegionSideExit};

/// Compiled host entry point owned by one Cranelift module generation.
#[derive(Debug)]
pub struct CraneliftMips4Block {
    entry: NonNull<u8>,
}

/// Compiled host entry point for one bounded MIPS IV Region.
#[derive(Debug)]
pub struct CraneliftMips4Region {
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
    type CompiledRegion = CraneliftMips4Region;
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
            .map_err(|error| CraneliftMips4Error::message(format!("{error:?}")))?;
        self.module.clear_context(&mut context);
        self.module
            .finalize_definitions()
            .map_err(CraneliftMips4Error::new)?;
        let entry = NonNull::new(self.module.get_finalized_function(function).cast_mut())
            .ok_or_else(|| CraneliftMips4Error::message("Cranelift returned a null entry point"))?;
        Ok(CraneliftMips4Block { entry })
    }

    fn execute<'fast, R>(
        &mut self,
        compiled: &Self::CompiledBlock,
        frame: &mut Mips4BlockFrame,
        runtime: &mut R,
        operations: &[Mips4RuntimeOperation],
        fast_memory: Option<&mut (dyn Mips4FastMemoryRuntime + 'fast)>,
    ) -> Result<Mips4BlockExit, Self::Error>
    where
        R: Mips4BlockRuntime,
    {
        type NativeBlock = unsafe extern "C" fn(*mut Mips4NativeFrame) -> u32;

        let mut invocation = Mips4NativeInvocation::new(frame, runtime, operations, fast_memory);
        // SAFETY: Cranelift emitted this entry with the exact NativeBlock signature,
        // and the module remains alive while every compiled handle is cached.
        let function: NativeBlock = unsafe { mem::transmute(compiled.entry.as_ptr()) };
        // SAFETY: The frame is uniquely borrowed for the duration of native execution.
        let code = unsafe { function(invocation.frame_mut_ptr()) };
        invocation.finish();
        block_exit_from_code(code).ok_or_else(|| {
            CraneliftMips4Error::message(format!("native MIPS IV block returned exit code {code}"))
        })
    }

    fn compile_region(
        &mut self,
        region: &Mips4Region,
    ) -> Result<Self::CompiledRegion, Self::Error> {
        region.verify().map_err(CraneliftMips4Error::new)?;
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
            lower_region(
                &mut builder,
                frame,
                region,
                pointer_type,
                self.module.target_config().default_call_conv,
            );
            builder.seal_all_blocks();
            builder.finalize();
        }

        let name = format!("mips4_region_{}", self.next_function);
        self.next_function = self.next_function.wrapping_add(1);
        let function = self
            .module
            .declare_function(&name, Linkage::Local, &context.func.signature)
            .map_err(CraneliftMips4Error::new)?;
        self.module
            .define_function(function, &mut context)
            .map_err(|error| CraneliftMips4Error::message(format!("{error:?}")))?;
        self.module.clear_context(&mut context);
        self.module
            .finalize_definitions()
            .map_err(CraneliftMips4Error::new)?;
        let entry = NonNull::new(self.module.get_finalized_function(function).cast_mut())
            .ok_or_else(|| CraneliftMips4Error::message("Cranelift returned a null entry point"))?;
        Ok(CraneliftMips4Region { entry })
    }

    fn execute_region<'fast, R>(
        &mut self,
        compiled: &Self::CompiledRegion,
        frame: &mut Mips4BlockFrame,
        runtime: &mut R,
        operations: &[Mips4RuntimeOperation],
        fast_memory: Option<&mut (dyn Mips4FastMemoryRuntime + 'fast)>,
    ) -> Result<(Mips4BlockExit, Option<Mips4RegionSideExit>), Self::Error>
    where
        R: Mips4BlockRuntime,
    {
        type NativeRegion = unsafe extern "C" fn(*mut Mips4NativeFrame) -> u32;

        let mut invocation = Mips4NativeInvocation::new(frame, runtime, operations, fast_memory);
        // SAFETY: Cranelift emitted this entry with the exact NativeRegion signature,
        // and the module remains alive while every compiled handle is cached.
        let function: NativeRegion = unsafe { mem::transmute(compiled.entry.as_ptr()) };
        // SAFETY: The frame is uniquely borrowed for the duration of native execution.
        let code = unsafe { function(invocation.frame_mut_ptr()) };
        let side_exit = invocation.region_side_exit();
        invocation.finish();
        block_exit_from_code(code)
            .map(|exit| (exit, side_exit))
            .ok_or_else(|| {
                CraneliftMips4Error::message(format!(
                    "native MIPS IV Region returned exit code {code}"
                ))
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
    lower_block_with_region_edge(
        builder,
        frame,
        block,
        pointer_type,
        call_conv,
        0,
        false,
        None,
    );
}

fn lower_region(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    region: &Mips4Region,
    pointer_type: cranelift_codegen::ir::Type,
    call_conv: CallConv,
) {
    let runtime_side_exit = builder.ins().iconst(
        types::I64,
        region_side_exit_code(Mips4RegionSideExit::Runtime) as i64,
    );
    store_i64(
        builder,
        frame,
        MIPS4_BLOCK_FRAME_REGION_SIDE_EXIT_OFFSET,
        runtime_side_exit,
    );
    let headers = region
        .nodes()
        .iter()
        .map(|_| builder.create_block())
        .collect::<Vec<_>>();
    builder.ins().jump(headers[0], &[]);
    let mut runtime_descriptor_base = 0_u32;
    for (node_index, node) in region.nodes().iter().enumerate() {
        builder.switch_to_block(headers[node_index]);
        let edge = node.hot_successor().map(|successor| NativeRegionEdge {
            header: headers[successor],
            entry: region.nodes()[successor].block().key(),
        });
        lower_block_with_region_edge(
            builder,
            frame,
            node.block(),
            pointer_type,
            call_conv,
            runtime_descriptor_base,
            true,
            edge,
        );
        runtime_descriptor_base =
            runtime_descriptor_base.saturating_add(block_runtime_operation_count(node.block()));
    }
}

#[derive(Clone, Copy)]
struct NativeRegionEdge {
    header: cranelift_codegen::ir::Block,
    entry: se_device::cpu::mips4::execution::block::Mips4BlockKey,
}

fn block_runtime_operation_count(block: &Mips4Block) -> u32 {
    block
        .body()
        .iter()
        .chain(block.delay_slot().iter())
        .filter(|instruction| matches!(instruction.operation, Mips4BlockOperation::Runtime(_)))
        .count() as u32
}

#[allow(clippy::too_many_arguments)]
fn lower_block_with_region_edge(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    block: &Mips4Block,
    pointer_type: cranelift_codegen::ir::Type,
    call_conv: CallConv,
    runtime_descriptor_base: u32,
    region_mode: bool,
    edge: Option<NativeRegionEdge>,
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
            store_accounting(builder, frame, accounting);
        }
        let runtime_operation = matches!(instruction.operation, Mips4BlockOperation::Runtime(_));
        match lower_operation(
            builder,
            frame,
            instruction.operation,
            &mut runtime_index,
            runtime_descriptor_base,
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
        lower_dispatch(
            builder,
            frame,
            entered_operations,
            control,
            accounting,
            region_mode,
            edge,
        );
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
    lower_dispatch(
        builder,
        frame,
        entered_operations,
        control,
        accounting,
        region_mode,
        edge,
    );
    builder.switch_to_block(delay);
    let Some(delay_slot) = block.delay_slot() else {
        lower_dispatch(
            builder,
            frame,
            entered_operations,
            control,
            accounting,
            region_mode,
            edge,
        );
        return;
    };
    entered_operations += 1;
    if operation_requires_early_count(delay_slot.operation) {
        lower_enter_operation(builder, frame, entered_operations);
        store_control(builder, frame, control);
        store_accounting(builder, frame, accounting);
    }
    let runtime_operation = matches!(delay_slot.operation, Mips4BlockOperation::Runtime(_));
    match lower_operation(
        builder,
        frame,
        delay_slot.operation,
        &mut runtime_index,
        runtime_descriptor_base,
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
    lower_dispatch(
        builder,
        frame,
        entered_operations,
        control,
        accounting,
        region_mode,
        edge,
    );
}

#[allow(clippy::too_many_arguments)]
fn lower_dispatch(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    entered_operations: u64,
    control: NativeControl,
    accounting: NativeAccounting,
    region_mode: bool,
    edge: Option<NativeRegionEdge>,
) {
    lower_enter_operation(builder, frame, entered_operations);
    store_accounting(builder, frame, accounting);
    store_control(builder, frame, control);
    let Some(edge) = edge else {
        if region_mode {
            store_region_side_exit(builder, frame, Mips4RegionSideExit::ColdSuccessor);
        }
        return_exit(builder, Mips4BlockExit::Dispatch);
        return;
    };

    let pc = builder
        .ins()
        .icmp_imm(IntCC::Equal, control.pc, edge.entry.pc as i64);
    let next_pc = builder
        .ins()
        .icmp_imm(IntCC::Equal, control.next_pc, edge.entry.next_pc as i64);
    let expected_delay_pc = edge.entry.delay_slot_branch_pc.unwrap_or(0);
    let delay_pc = builder
        .ins()
        .icmp_imm(IntCC::Equal, control.delay_pc, expected_delay_pc as i64);
    let delay_valid = builder.ins().icmp_imm(
        IntCC::Equal,
        control.delay_valid,
        i64::from(edge.entry.delay_slot_branch_pc.is_some()),
    );
    let control_matches = builder.ins().band(pc, next_pc);
    let control_matches = builder.ins().band(control_matches, delay_pc);
    let control_matches = builder.ins().band(control_matches, delay_valid);
    let hot = builder.create_block();
    let cold = builder.create_block();
    builder.ins().brif(control_matches, hot, &[], cold, &[]);

    builder.switch_to_block(hot);
    add_region_base(
        builder,
        frame,
        MIPS4_BLOCK_FRAME_OPERATION_BASE_OFFSET,
        entered_operations,
    );
    builder.ins().jump(edge.header, &[]);

    builder.switch_to_block(cold);
    if region_mode {
        store_region_side_exit(builder, frame, Mips4RegionSideExit::ColdSuccessor);
    }
    return_exit(builder, Mips4BlockExit::Dispatch);
}

fn store_region_side_exit(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    side_exit: Mips4RegionSideExit,
) {
    let side_exit = builder
        .ins()
        .iconst(types::I64, region_side_exit_code(side_exit) as i64);
    store_i64(
        builder,
        frame,
        MIPS4_BLOCK_FRAME_REGION_SIDE_EXIT_OFFSET,
        side_exit,
    );
}

fn store_region_budget_side_exit(builder: &mut FunctionBuilder<'_>, frame: Value) {
    let current = load_i64(builder, frame, MIPS4_BLOCK_FRAME_REGION_SIDE_EXIT_OFFSET);
    let region_active = builder.ins().icmp_imm(IntCC::NotEqual, current, 0);
    let budget = builder.ins().iconst(
        types::I64,
        region_side_exit_code(Mips4RegionSideExit::Budget) as i64,
    );
    let side_exit = builder.ins().select(region_active, budget, current);
    store_i64(
        builder,
        frame,
        MIPS4_BLOCK_FRAME_REGION_SIDE_EXIT_OFFSET,
        side_exit,
    );
}

fn add_region_base(builder: &mut FunctionBuilder<'_>, frame: Value, offset: i32, increment: u64) {
    let base = load_i64(builder, frame, offset);
    let base = builder.ins().iadd_imm(base, increment as i64);
    store_i64(builder, frame, offset, base);
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

#[allow(clippy::too_many_arguments)]
fn lower_operation(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    operation: Mips4BlockOperation,
    runtime_index: &mut u32,
    runtime_descriptor_base: u32,
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
            let local_index = *runtime_index;
            *runtime_index = runtime_index.saturating_add(1);
            let descriptor_index = runtime_descriptor_base.saturating_add(local_index);
            if let Mips4RuntimeOperation::Memory { instruction, raw } = operation
                && let Some((size, signed)) = fast_integer_load_shape(instruction)
            {
                lower_fast_integer_load(
                    builder,
                    frame,
                    descriptor_index,
                    operation,
                    raw.rs(),
                    raw.rt(),
                    raw.signed_immediate(),
                    size,
                    signed,
                    entered_operations,
                    accounting,
                    runtime_abi,
                );
            } else {
                let allow_fast_memory = builder.ins().iconst(types::I32, 1);
                lower_runtime_operation(
                    builder,
                    accounting,
                    RuntimeOperationLowering {
                        frame,
                        operation: descriptor_index,
                        runtime_operation: operation,
                        entered_operations,
                        runtime_abi,
                        allow_fast_memory,
                    },
                );
            }
            return LoweredOperation::Retired;
        }
        Mips4BlockOperation::NoOperation => {}
    }
    LoweredOperation::NeedsRetirement
}

const fn fast_integer_load_shape(instruction: Mips4CpuInstruction) -> Option<(u8, bool)> {
    match instruction {
        Mips4CpuInstruction::Lb => Some((1, true)),
        Mips4CpuInstruction::Lbu => Some((1, false)),
        Mips4CpuInstruction::Lh => Some((2, true)),
        Mips4CpuInstruction::Lhu => Some((2, false)),
        Mips4CpuInstruction::Lw => Some((4, true)),
        Mips4CpuInstruction::Lwu => Some((4, false)),
        Mips4CpuInstruction::Ld => Some((8, false)),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn lower_fast_integer_load(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    operation_index: u32,
    operation: Mips4RuntimeOperation,
    base_register: u8,
    target_register: u8,
    offset: i16,
    size: u8,
    signed: bool,
    entered_operations: u64,
    accounting: &mut NativeAccounting,
    runtime_abi: NativeRuntimeAbi,
) {
    const KSEG1_START: u64 = 0xffff_ffff_a000_0000;
    const KSEG1_END: u64 = 0xffff_ffff_bfff_ffff;

    let base = load_gpr(builder, frame, base_register);
    let virtual_address = builder.ins().iadd_imm(base, i64::from(offset));
    let kseg1_start = builder.ins().iconst(types::I64, KSEG1_START as i64);
    let physical_address = builder.ins().isub(virtual_address, kseg1_start);
    let aligned_bits = builder.ins().band_imm(virtual_address, i64::from(size - 1));
    let aligned = builder.ins().icmp_imm(IntCC::Equal, aligned_bits, 0);
    let above_start = builder.ins().icmp_imm(
        IntCC::UnsignedGreaterThanOrEqual,
        virtual_address,
        KSEG1_START as i64,
    );
    let below_end = builder.ins().icmp_imm(
        IntCC::UnsignedLessThanOrEqual,
        virtual_address,
        KSEG1_END as i64,
    );
    let read_entry = builder.ins().load(
        runtime_abi.pointer_type,
        MemFlagsData::trusted(),
        frame,
        MIPS4_BLOCK_FRAME_FAST_MEMORY_READ_OFFSET,
    );
    let entry_available = builder.ins().icmp_imm(IntCC::NotEqual, read_entry, 0);
    let mut eligible = builder.ins().band(aligned, above_start);
    eligible = builder.ins().band(eligible, below_end);
    eligible = builder.ins().band(eligible, entry_available);

    let direct = builder.create_block();
    let fallback = builder.create_block();
    builder.append_block_param(fallback, types::I32);
    let completed = builder.create_block();
    let commit = builder.create_block();
    let classify_failure = builder.create_block();
    let timeline_exhausted = builder.create_block();
    let invalid = builder.create_block();
    let done = builder.create_block();
    let allow_fast_memory = builder.ins().iconst(types::I32, 1);
    let suppress_fast_memory = builder.ins().iconst(types::I32, 0);
    let allow_argument = [BlockArg::Value(allow_fast_memory)];
    builder
        .ins()
        .brif(eligible, direct, &[], fallback, &allow_argument);

    builder.switch_to_block(direct);
    record_runtime_call(builder, frame);
    let context = builder.ins().load(
        runtime_abi.pointer_type,
        MemFlagsData::trusted(),
        frame,
        MIPS4_BLOCK_FRAME_FAST_MEMORY_CONTEXT_OFFSET,
    );
    let retired = load_i64(builder, frame, MIPS4_BLOCK_FRAME_RETIRED_OFFSET);
    let size_value = builder.ins().iconst(types::I32, i64::from(size));
    let result_pointer = builder.ins().iadd_imm(
        frame,
        i64::from(MIPS4_BLOCK_FRAME_FAST_MEMORY_RESULT_OFFSET),
    );
    let mut signature = Signature::new(runtime_abi.call_conv);
    signature.params.extend([
        AbiParam::new(runtime_abi.pointer_type),
        AbiParam::new(types::I64),
        AbiParam::new(types::I64),
        AbiParam::new(types::I32),
        AbiParam::new(runtime_abi.pointer_type),
    ]);
    signature.returns.push(AbiParam::new(types::I32));
    let signature = builder.import_signature(signature);
    let call = builder.ins().call_indirect(
        signature,
        read_entry,
        &[
            context,
            physical_address,
            retired,
            size_value,
            result_pointer,
        ],
    );
    let result = builder.inst_results(call)[0];
    let complete = builder.ins().icmp_imm(IntCC::Equal, result, 1);
    builder
        .ins()
        .brif(complete, completed, &[], classify_failure, &[]);

    builder.switch_to_block(completed);
    let retirement_limit = load_i64(
        builder,
        result_pointer,
        MIPS4_FAST_MEMORY_RESULT_RETIREMENT_LIMIT_OFFSET,
    );
    let limit_valid = builder
        .ins()
        .icmp(IntCC::UnsignedGreaterThan, retirement_limit, retired);
    builder.ins().brif(limit_valid, commit, &[], invalid, &[]);

    builder.switch_to_block(commit);
    let remaining = builder.ins().isub(retirement_limit, retired);
    let current_budget = load_i64(builder, frame, MIPS4_BLOCK_FRAME_BUDGET_OFFSET);
    let tighter = builder
        .ins()
        .icmp(IntCC::UnsignedLessThan, remaining, current_budget);
    let budget = builder.ins().select(tighter, remaining, current_budget);
    store_i64(builder, frame, MIPS4_BLOCK_FRAME_BUDGET_OFFSET, budget);
    let lanes = load_i64(
        builder,
        result_pointer,
        MIPS4_FAST_MEMORY_RESULT_VALUE_OFFSET,
    );
    let big_endian = load_i64(
        builder,
        frame,
        MIPS4_BLOCK_FRAME_RUNTIME_MEMORY_BIG_ENDIAN_OFFSET,
    );
    let big_endian = builder.ins().icmp_imm(IntCC::NotEqual, big_endian, 0);
    let value = lower_integer_load_value(builder, lanes, big_endian, size, signed);
    store_gpr(builder, frame, target_register, value);
    lower_retire_sequential(builder, frame);
    let runtime_accounting = load_accounting(builder, frame);
    let continued_accounting = retire_accounting(builder, runtime_accounting);
    lower_budget_check(builder, frame, entered_operations, continued_accounting);
    store_accounting(builder, frame, continued_accounting);
    builder.ins().jump(done, &[]);

    builder.switch_to_block(classify_failure);
    let unavailable = builder.ins().icmp_imm(IntCC::Equal, result, 0);
    let classify_nonzero = builder.create_block();
    builder.ins().brif(
        unavailable,
        fallback,
        &[BlockArg::Value(suppress_fast_memory)],
        classify_nonzero,
        &[],
    );
    builder.switch_to_block(classify_nonzero);
    let exhausted = builder.ins().icmp_imm(IntCC::Equal, result, 2);
    builder
        .ins()
        .brif(exhausted, timeline_exhausted, &[], invalid, &[]);

    builder.switch_to_block(timeline_exhausted);
    lower_enter_operation(builder, frame, entered_operations.saturating_sub(1));
    return_exit(builder, Mips4BlockExit::TimelineExhausted);

    builder.switch_to_block(invalid);
    return_exit(builder, Mips4BlockExit::InternalError);

    builder.switch_to_block(fallback);
    let allow_fast_memory = builder.block_params(fallback)[0];
    lower_runtime_operation(
        builder,
        accounting,
        RuntimeOperationLowering {
            frame,
            operation: operation_index,
            runtime_operation: operation,
            entered_operations,
            runtime_abi,
            allow_fast_memory,
        },
    );
    store_accounting(builder, frame, *accounting);
    builder.ins().jump(done, &[]);

    builder.switch_to_block(done);
    *accounting = load_accounting(builder, frame);
}
fn lower_integer_load_value(
    builder: &mut FunctionBuilder<'_>,
    lanes: Value,
    big_endian: Value,
    size: u8,
    signed: bool,
) -> Value {
    let lane_type = match size {
        1 => types::I8,
        2 => types::I16,
        4 => types::I32,
        8 => types::I64,
        _ => unreachable!("typed integer loads have a supported width"),
    };
    let lanes = if size == 8 {
        lanes
    } else {
        builder.ins().ireduce(lane_type, lanes)
    };
    let decoded = if size == 1 {
        lanes
    } else {
        let swapped = builder.ins().bswap(lanes);
        builder.ins().select(big_endian, swapped, lanes)
    };
    if size == 8 {
        decoded
    } else if signed {
        builder.ins().sextend(types::I64, decoded)
    } else {
        builder.ins().uextend(types::I64, decoded)
    }
}

#[derive(Clone, Copy)]
struct RuntimeOperationLowering {
    frame: Value,
    operation: u32,
    runtime_operation: Mips4RuntimeOperation,
    entered_operations: u64,
    runtime_abi: NativeRuntimeAbi,
    allow_fast_memory: Value,
}

fn lower_runtime_operation(
    builder: &mut FunctionBuilder<'_>,
    accounting: &mut NativeAccounting,
    lowering: RuntimeOperationLowering,
) {
    let RuntimeOperationLowering {
        frame,
        operation,
        runtime_operation,
        entered_operations,
        runtime_abi,
        allow_fast_memory,
    } = lowering;
    record_runtime_call(builder, frame);
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
        AbiParam::new(types::I32),
    ]);
    signature.returns.push(AbiParam::new(types::I32));
    let signature = builder.import_signature(signature);
    let call = builder.ins().call_indirect(
        signature,
        runtime_call,
        &[context, frame, operation, allow_fast_memory],
    );
    let result = builder.inst_results(call)[0];
    let runtime_accounting = load_accounting(builder, frame);
    let continued_accounting = retire_accounting(builder, runtime_accounting);

    let expected_result = expected_runtime_result(runtime_operation);
    let expected = builder.ins().icmp_imm(
        IntCC::Equal,
        result,
        i64::from(runtime_result_code(expected_result)),
    );
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
    let timeline_exhausted = builder.create_block();
    let invalid = builder.create_block();
    let done = builder.create_block();
    if expected_result == Mips4RuntimeResult::ContinueControl {
        builder.ins().jump(done, &[]);
    }
    builder.switch_to_block(uncommon_block);
    let rare_block = if let Some(secondary_result) = secondary_runtime_result(runtime_operation) {
        let secondary = builder.ins().icmp_imm(
            IntCC::Equal,
            result,
            i64::from(runtime_result_code(secondary_result)),
        );
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
    switch.set_entry(
        u128::from(runtime_result_code(Mips4RuntimeResult::Continue)),
        continue_sequential,
    );
    switch.set_entry(
        u128::from(runtime_result_code(Mips4RuntimeResult::ContinueControl)),
        continue_control,
    );
    switch.set_entry(
        u128::from(runtime_result_code(Mips4RuntimeResult::DispatchSequential)),
        dispatch_sequential,
    );
    switch.set_entry(
        u128::from(runtime_result_code(Mips4RuntimeResult::DispatchControl)),
        dispatch_control,
    );
    switch.set_entry(
        u128::from(runtime_result_code(Mips4RuntimeResult::Transaction)),
        transaction,
    );
    switch.set_entry(
        u128::from(runtime_result_code(Mips4RuntimeResult::Exception)),
        exception,
    );
    switch.set_entry(
        u128::from(runtime_result_code(Mips4RuntimeResult::Idle)),
        idle,
    );
    switch.set_entry(
        u128::from(runtime_result_code(Mips4RuntimeResult::TimelineExhausted)),
        timeline_exhausted,
    );
    switch.set_entry(
        u128::from(runtime_result_code(Mips4RuntimeResult::InternalError)),
        invalid,
    );
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

    builder.switch_to_block(timeline_exhausted);
    lower_enter_operation(builder, frame, entered_operations.saturating_sub(1));
    store_accounting(builder, frame, *accounting);
    return_exit(builder, Mips4BlockExit::TimelineExhausted);

    builder.switch_to_block(invalid);
    return_exit(builder, Mips4BlockExit::InternalError);

    builder.switch_to_block(done);
    *accounting = continued_accounting;
}

fn record_runtime_call(builder: &mut FunctionBuilder<'_>, frame: Value) {
    let runtime_calls = load_i64(builder, frame, MIPS4_BLOCK_FRAME_RUNTIME_CALLS_OFFSET);
    let runtime_calls = builder.ins().iadd_imm(runtime_calls, 1);
    store_i64(
        builder,
        frame,
        MIPS4_BLOCK_FRAME_RUNTIME_CALLS_OFFSET,
        runtime_calls,
    );
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
    let operation_base = load_i64(builder, frame, MIPS4_BLOCK_FRAME_OPERATION_BASE_OFFSET);
    let entered = builder
        .ins()
        .iadd_imm(operation_base, entered_operations as i64);
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
    store_region_budget_side_exit(builder, frame);
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
    store_region_budget_side_exit(builder, frame);
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
    let exception = builder
        .ins()
        .iconst(types::I64, block_exception_code(exception) as i64);
    store_i64(
        builder,
        frame,
        MIPS4_BLOCK_FRAME_EXCEPTION_OFFSET,
        exception,
    );
}

fn return_exit(builder: &mut FunctionBuilder<'_>, exit: Mips4BlockExit) {
    let exit = builder
        .ins()
        .iconst(types::I32, i64::from(block_exit_code(exit)));
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
        Mips4BlockBranch, Mips4BlockGuard, Mips4BlockInstruction, Mips4BlockInstructionMetadata,
        Mips4BlockKey, Mips4BlockLiftedInstruction, Mips4BlockRetire, Mips4BlockRuntime,
        Mips4CodeGuard, Mips4CodeSourceId, Mips4FastMemoryReadRequest, Mips4FastMemoryReadResult,
        Mips4FastMemoryRuntime, Mips4RuntimeOperation, interpret_block, lift_cpu_instruction,
    };
    use se_device::cpu::mips4::instruction::Mips4Instruction;
    use se_device::cpu::mips4::instruction::decode::{
        Mips4InstructionClass, Mips4InstructionDecode, decode_instruction,
    };
    use se_device::cpu::mips4::model::r5000::boot_mode::R5000BootMode;
    use se_device::cpu::mips4::model::r5000::execution_policy::R5000ExecutionPolicy;
    use se_device::cpu::mips4::model::r5000::profile::R5000Profile;
    use se_device::cpu::mips4::model::r5000::revision::R5000Revision;

    use crate::mips4::engine::{Mips4BlockEngine, Mips4BlockTier};
    use crate::mips4::region::Mips4RegionNode;

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
        let mut runtime = RejectRuntime;
        let operations = block.runtime_operations();
        let native_exit = backend
            .execute(&compiled, &mut native, &mut runtime, &operations, None)
            .unwrap();
        assert_eq!(native_exit, interpreted_exit);
        assert_eq!(native, interpreted);
    }

    #[test]
    fn unavailable_native_fast_read_is_not_retried_by_the_runtime_fallback() {
        struct UnavailableFastMemory {
            attempts: u64,
        }

        impl Mips4FastMemoryRuntime for UnavailableFastMemory {
            fn read(&mut self, request: Mips4FastMemoryReadRequest) -> Mips4FastMemoryReadResult {
                assert_eq!(request.physical_address(), 0x1000);
                assert_eq!(request.size(), 4);
                assert_eq!(request.retired_boundaries(), 0);
                self.attempts += 1;
                Mips4FastMemoryReadResult::Unavailable
            }
        }

        struct FallbackRuntime {
            calls: u64,
        }

        impl Mips4BlockRuntime for FallbackRuntime {
            fn execute<F>(
                &mut self,
                frame: &mut Mips4BlockFrame,
                operation: Mips4RuntimeOperation,
                fast_memory: Option<&mut F>,
            ) -> Mips4RuntimeResult
            where
                F: Mips4FastMemoryRuntime + ?Sized,
            {
                assert!(matches!(operation, Mips4RuntimeOperation::Memory { .. }));
                assert!(fast_memory.is_none());
                self.calls += 1;
                frame.write_gpr(3, 0x1122_3344);
                Mips4RuntimeResult::Continue
            }
        }

        let key = Mips4BlockKey {
            pc: 0x2000,
            next_pc: 0x2004,
            delay_slot_branch_pc: None,
            fetch_context: 0,
            translation_generation: 0,
            code_guard: 0,
        };
        let mut block = Mips4Block::new(key, Mips4BlockGuard::new());
        block.push(sequential(0x2000, 0x8c23_0000)).unwrap();
        block.terminate_dispatch().unwrap();
        let mut backend = CraneliftMips4Backend::new().unwrap();
        let compiled = backend.compile(&block).unwrap();
        let mut gpr = [0; 32];
        gpr[1] = 0xffff_ffff_a000_1000;
        let mut frame = Mips4BlockFrame::new(gpr, 0, 0, 0x2000, 0x2004, None, 1);
        let mut runtime = FallbackRuntime { calls: 0 };
        let mut fast_memory = UnavailableFastMemory { attempts: 0 };
        let operations = block.runtime_operations();
        let exit = backend
            .execute(
                &compiled,
                &mut frame,
                &mut runtime,
                &operations,
                Some(&mut fast_memory),
            )
            .unwrap();
        assert_eq!(exit, Mips4BlockExit::BudgetExhausted);
        assert_eq!(frame.read_gpr(3), 0x1122_3344);
        assert_eq!(fast_memory.attempts, 1);
        assert_eq!(runtime.calls, 1);
        assert_eq!(frame.runtime_calls(), 2);
    }

    #[test]
    fn native_multi_block_region_matches_boundary_interpreter() {
        let code_guard = Mips4CodeGuard {
            source_id: Mips4CodeSourceId::new(1),
            source_offset: 0,
            revision: 1,
            fingerprint: 2,
        };
        let first_key = Mips4BlockKey {
            pc: 0x1000,
            next_pc: 0x1004,
            delay_slot_branch_pc: None,
            fetch_context: 7,
            translation_generation: 11,
            code_guard: code_guard.token(),
        };
        let second_key = Mips4BlockKey {
            pc: 0x1004,
            next_pc: 0x1008,
            ..first_key
        };
        let mut first = Mips4Block::new(first_key, Mips4BlockGuard::from_code_source(code_guard));
        first
            .push(sequential(
                0x1000,
                (0x09_u32 << 26) | (1 << 21) | (1 << 16) | 1,
            ))
            .unwrap();
        first.terminate_dispatch().unwrap();

        let mut second = Mips4Block::new(second_key, Mips4BlockGuard::from_code_source(code_guard));
        second
            .terminate_with_branch(
                branch(0x1004, (0x04_u32 << 26) | u32::from(u16::MAX - 1)),
                sequential_with_delay(0x1008, 0, Some(0x1004)),
            )
            .unwrap();

        let region = Mips4Region::new(vec![
            Mips4RegionNode::new(first.clone(), Some(1)),
            Mips4RegionNode::new(second.clone(), Some(0)),
        ])
        .unwrap();
        let mut backend = CraneliftMips4Backend::new().unwrap();
        let compiled = backend.compile_region(&region).unwrap();

        for budget in 1..=32 {
            let frame = Mips4BlockFrame::new([0; 32], 0, 0, 0x1000, 0x1004, None, budget);
            let mut interpreted = frame.clone();
            let mut interpreted_operations = 0_u64;
            loop {
                let block = match interpreted.pc() {
                    0x1000 => &first,
                    0x1004 => &second,
                    pc => panic!("unexpected interpreted Region PC {pc:#x}"),
                };
                let exit = interpret_block(block, &mut interpreted);
                interpreted_operations =
                    interpreted_operations.saturating_add(interpreted.operations_executed());
                match exit {
                    Mips4BlockExit::Dispatch => {}
                    Mips4BlockExit::BudgetExhausted => break,
                    other => panic!("unexpected interpreted Region exit {other:?}"),
                }
            }

            let mut native = frame;
            let mut runtime = RejectRuntime;
            let operations = region.runtime_operations();
            let (exit, side_exit) = backend
                .execute_region(&compiled, &mut native, &mut runtime, &operations, None)
                .unwrap();
            assert_eq!(exit, Mips4BlockExit::BudgetExhausted);
            assert_eq!(native.gpr(), interpreted.gpr());
            assert_eq!(native.hi(), interpreted.hi());
            assert_eq!(native.lo(), interpreted.lo());
            assert_eq!(native.pc(), interpreted.pc());
            assert_eq!(native.next_pc(), interpreted.next_pc());
            assert_eq!(
                native.delay_slot_branch_pc(),
                interpreted.delay_slot_branch_pc()
            );
            assert_eq!(native.budget(), interpreted.budget());
            assert_eq!(native.retired(), interpreted.retired());
            assert_eq!(native.exception(), interpreted.exception());
            assert_eq!(native.operations_executed(), interpreted_operations);
            assert_eq!(native.runtime_calls(), 0);
            assert_eq!(side_exit, Some(Mips4RegionSideExit::Budget));
        }
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
        let mut runtime = RejectRuntime;
        let operations = block.runtime_operations();
        assert_eq!(
            backend
                .execute(&compiled, &mut native, &mut runtime, &operations, None)
                .unwrap(),
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
            let mut runtime = RejectRuntime;
            let operations = block.runtime_operations();
            let native_exit = backend
                .execute(&compiled, &mut native, &mut runtime, &operations, None)
                .unwrap();
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
            fn execute<F>(
                &mut self,
                frame: &mut Mips4BlockFrame,
                operation: Mips4RuntimeOperation,
                _fast_memory: Option<&mut F>,
            ) -> Mips4RuntimeResult
            where
                F: Mips4FastMemoryRuntime + ?Sized,
            {
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
                .execute_with_runtime(key, &mut frame, &mut runtime, None)
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
            .execute_with_runtime(key, &mut frame, &mut runtime, None)
            .unwrap();
        assert_eq!(execution.tier, Mips4BlockTier::Native);
        assert_eq!(execution.exit, Mips4BlockExit::RuntimeIdle);
        assert_eq!(frame.retired(), 1);
        assert_eq!(frame.pc(), 0x4000);
    }
}
