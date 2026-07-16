use std::hint::black_box;
use std::time::Instant;

use se_device::cpu::mips4::config::{Mips4CacheConfig, Mips4Endianness};
use se_device::cpu::mips4::execution::block::{
    Mips4Block, Mips4BlockFrame, Mips4BlockGuard, Mips4BlockInstructionMetadata, Mips4BlockKey,
    Mips4BlockLiftedInstruction, Mips4BlockRuntime, Mips4CodeGuard, Mips4CodeGuardKind,
    Mips4FastMemoryRuntime, Mips4RuntimeOperation, Mips4RuntimeResult, interpret_block,
    lift_cpu_instruction,
};
use se_device::cpu::mips4::instruction::Mips4Instruction;
use se_device::cpu::mips4::instruction::decode::{
    Mips4InstructionClass, Mips4InstructionDecode, decode_instruction,
};
use se_device::cpu::mips4::model::r5000::boot_mode::R5000BootMode;
use se_device::cpu::mips4::model::r5000::execution_policy::R5000ExecutionPolicy;
use se_device::cpu::mips4::model::r5000::profile::R5000Profile;
use se_device::cpu::mips4::model::r5000::revision::R5000Revision;
use se_jit::mips4::cranelift::{CraneliftMips4Backend, CraneliftMips4Block, CraneliftMips4Region};
use se_jit::mips4::engine::Mips4CodegenBackend;
use se_jit::mips4::region::{Mips4Region, Mips4RegionNode};

const BASE: u64 = 0x1000;
const ITERATIONS: usize = 500_000;
const REGION_ITERATIONS: usize = 100_000;
const REGION_BUDGET: u64 = 100;

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

#[inline(always)]
fn execute_native_block(
    backend: &mut CraneliftMips4Backend,
    compiled: &CraneliftMips4Block,
    frame: &mut Mips4BlockFrame,
    runtime: &mut RejectRuntime,
) {
    black_box(
        backend
            .execute(compiled, frame, runtime, &[], None)
            .unwrap(),
    );
}

#[inline(always)]
fn execute_native_region(
    backend: &mut CraneliftMips4Backend,
    compiled: &CraneliftMips4Region,
    frame: &mut Mips4BlockFrame,
    runtime: &mut RejectRuntime,
) {
    black_box(
        backend
            .execute_region(compiled, frame, runtime, &[], None)
            .unwrap(),
    );
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

fn lift(pc: u64, bits: u32, delay_slot_branch_pc: Option<u64>) -> Mips4BlockLiftedInstruction {
    let raw = Mips4Instruction::from_bits(bits);
    let Mips4InstructionDecode::Instruction(Mips4InstructionClass::Cpu(decoded)) =
        decode_instruction(raw)
    else {
        panic!("benchmark instruction must decode as CPU")
    };
    lift_cpu_instruction(
        &policy(),
        Mips4BlockInstructionMetadata {
            pc,
            instruction: bits,
            delay_slot_branch_pc,
        },
        decoded,
    )
}

fn hot_loop() -> Mips4Block {
    let code_guard = benchmark_code_guard();
    let key = Mips4BlockKey {
        pc: BASE,
        next_pc: BASE + 4,
        delay_slot_branch_pc: None,
        fetch_context: 0,
        translation_generation: 0,
        code_guard: code_guard.token(),
    };
    let mut block = Mips4Block::new(key, Mips4BlockGuard::from_code_source(code_guard));
    let addiu = (0x09_u32 << 26) | (1 << 21) | (1 << 16) | 1;
    for index in 0..30 {
        let Mips4BlockLiftedInstruction::Sequential(instruction) =
            lift(BASE + index * 4, addiu, None)
        else {
            panic!("ADDIU must be sequential")
        };
        block.push(instruction).unwrap();
    }
    let branch_pc = BASE + 30 * 4;
    let beq = (0x04_u32 << 26) | 0xffe1;
    let Mips4BlockLiftedInstruction::Branch(branch) = lift(branch_pc, beq, None) else {
        panic!("BEQ must terminate the block")
    };
    let Mips4BlockLiftedInstruction::Sequential(delay_slot) =
        lift(branch_pc + 4, addiu, Some(branch_pc))
    else {
        panic!("delay ADDIU must be sequential")
    };
    block.terminate_with_branch(branch, delay_slot).unwrap();
    block
}

const fn benchmark_code_guard() -> Mips4CodeGuard {
    Mips4CodeGuard {
        kind: Mips4CodeGuardKind::SystemFlash,
        source_offset: 0,
        revision: 1,
        fingerprint: 2,
    }
}

fn region_loop() -> (Vec<Mips4Block>, Mips4Region) {
    let code_guard = benchmark_code_guard();
    let addiu = (0x09_u32 << 26) | (1 << 21) | (1 << 16) | 1;
    let mut blocks = Vec::new();
    for index in 0..3_u64 {
        let pc = BASE + index * 4;
        let key = Mips4BlockKey {
            pc,
            next_pc: pc + 4,
            delay_slot_branch_pc: None,
            fetch_context: 0,
            translation_generation: 0,
            code_guard: code_guard.token(),
        };
        let mut block = Mips4Block::new(key, Mips4BlockGuard::from_code_source(code_guard));
        let Mips4BlockLiftedInstruction::Sequential(instruction) = lift(pc, addiu, None) else {
            panic!("ADDIU must be sequential")
        };
        block.push(instruction).unwrap();
        block.terminate_dispatch().unwrap();
        blocks.push(block);
    }

    let branch_pc = BASE + 12;
    let branch_key = Mips4BlockKey {
        pc: branch_pc,
        next_pc: branch_pc + 4,
        delay_slot_branch_pc: None,
        fetch_context: 0,
        translation_generation: 0,
        code_guard: code_guard.token(),
    };
    let mut branch_block =
        Mips4Block::new(branch_key, Mips4BlockGuard::from_code_source(code_guard));
    let beq = (0x04_u32 << 26) | 0xfffc;
    let Mips4BlockLiftedInstruction::Branch(branch) = lift(branch_pc, beq, None) else {
        panic!("BEQ must terminate the block")
    };
    let Mips4BlockLiftedInstruction::Sequential(delay_slot) =
        lift(branch_pc + 4, addiu, Some(branch_pc))
    else {
        panic!("delay ADDIU must be sequential")
    };
    branch_block
        .terminate_with_branch(branch, delay_slot)
        .unwrap();
    blocks.push(branch_block);

    let nodes = blocks
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, block)| Mips4RegionNode::new(block, Some((index + 1) % 4)))
        .collect();
    let region = Mips4Region::new(nodes).unwrap();
    (blocks, region)
}

fn main() {
    let block = hot_loop();
    let frame = Mips4BlockFrame::new([0; 32], 0, 0, BASE, BASE + 4, None, 64);
    let mut interpreted = frame.clone();
    let started = Instant::now();
    for _ in 0..ITERATIONS {
        interpreted.prepare(64);
        black_box(interpret_block(
            black_box(&block),
            black_box(&mut interpreted),
        ));
    }
    let interpreted_time = started.elapsed();

    let mut backend = CraneliftMips4Backend::new().unwrap();
    let compiled = backend.compile(&block).unwrap();
    let mut native = frame;
    let mut runtime = RejectRuntime;
    for _ in 0..1_000 {
        native.prepare(64);
        execute_native_block(&mut backend, &compiled, &mut native, &mut runtime);
    }
    let started = Instant::now();
    for _ in 0..ITERATIONS {
        native.prepare(64);
        execute_native_block(
            &mut backend,
            black_box(&compiled),
            black_box(&mut native),
            &mut runtime,
        );
    }
    let native_time = started.elapsed();

    let (region_blocks, region) = region_loop();
    let compiled_region_blocks: Vec<_> = region_blocks
        .iter()
        .map(|block| backend.compile(block).unwrap())
        .collect();
    let compiled_region = backend.compile_region(&region).unwrap();
    let mut block_native = Mips4BlockFrame::new([0; 32], 0, 0, BASE, BASE + 4, None, REGION_BUDGET);
    let mut region_native = block_native.clone();
    for _ in 0..1_000 {
        block_native.prepare(REGION_BUDGET);
        while block_native.budget() != 0 {
            let index = usize::try_from((block_native.pc() - BASE) / 4).unwrap();
            execute_native_block(
                &mut backend,
                &compiled_region_blocks[index],
                &mut block_native,
                &mut runtime,
            );
        }
        region_native.prepare(REGION_BUDGET);
        execute_native_region(
            &mut backend,
            &compiled_region,
            &mut region_native,
            &mut runtime,
        );
    }
    let started = Instant::now();
    for _ in 0..REGION_ITERATIONS {
        block_native.prepare(REGION_BUDGET);
        while block_native.budget() != 0 {
            let index = usize::try_from((block_native.pc() - BASE) / 4).unwrap();
            execute_native_block(
                &mut backend,
                black_box(&compiled_region_blocks[index]),
                black_box(&mut block_native),
                &mut runtime,
            );
        }
    }
    let region_block_time = started.elapsed();
    let started = Instant::now();
    for _ in 0..REGION_ITERATIONS {
        region_native.prepare(REGION_BUDGET);
        execute_native_region(
            &mut backend,
            black_box(&compiled_region),
            black_box(&mut region_native),
            &mut runtime,
        );
    }
    let region_time = started.elapsed();

    let native_speedup = interpreted_time.as_secs_f64() / native_time.as_secs_f64();
    let region_speedup = region_block_time.as_secs_f64() / region_time.as_secs_f64();
    println!(
        "MIPS IV hot loop: interpreter={interpreted_time:?}, block={native_time:?}, block-speedup={native_speedup:.2}x; cross-block loop: blocks={region_block_time:?}, region={region_time:?}, region-speedup={region_speedup:.2}x"
    );
    assert!(
        native_speedup >= 1.5,
        "Cranelift JIT speedup {native_speedup:.2}x is below the required 1.5x"
    );
    assert!(
        region_time < region_block_time,
        "Cranelift Region execution did not outperform equal-work block dispatch"
    );
}
