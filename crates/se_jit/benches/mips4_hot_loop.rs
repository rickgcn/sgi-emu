use std::hint::black_box;
use std::time::Instant;

use se_device::cpu::mips4::config::{Mips4CacheConfig, Mips4Endianness};
use se_device::cpu::mips4::execution::block::{
    Mips4Block, Mips4BlockFrame, Mips4BlockGuard, Mips4BlockInstructionMetadata, Mips4BlockKey,
    Mips4BlockLiftedInstruction, Mips4CodegenBackend, interpret_block, lift_cpu_instruction,
};
use se_device::cpu::mips4::instruction::Mips4Instruction;
use se_device::cpu::mips4::instruction::decode::{
    Mips4InstructionClass, Mips4InstructionDecode, decode_instruction,
};
use se_device::cpu::mips4::model::r5000::boot_mode::R5000BootMode;
use se_device::cpu::mips4::model::r5000::execution_policy::R5000ExecutionPolicy;
use se_device::cpu::mips4::model::r5000::profile::R5000Profile;
use se_device::cpu::mips4::model::r5000::revision::R5000Revision;
use se_jit::mips4::CraneliftMips4Backend;

const BASE: u64 = 0x1000;
const ITERATIONS: usize = 500_000;

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
    let key = Mips4BlockKey {
        pc: BASE,
        next_pc: BASE + 4,
        delay_slot_branch_pc: None,
        fetch_context: 0,
        translation_generation: 0,
        code_guard: 0,
    };
    let mut block = Mips4Block::new(key, Mips4BlockGuard::new());
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
    for _ in 0..1_000 {
        native.prepare(64);
        black_box(backend.execute(&compiled, &mut native).unwrap());
    }
    let started = Instant::now();
    for _ in 0..ITERATIONS {
        native.prepare(64);
        black_box(
            backend
                .execute(black_box(&compiled), black_box(&mut native))
                .unwrap(),
        );
    }
    let native_time = started.elapsed();
    let speedup = interpreted_time.as_secs_f64() / native_time.as_secs_f64();
    println!(
        "MIPS IV hot loop: interpreter={interpreted_time:?}, JIT={native_time:?}, speedup={speedup:.2}x"
    );
    assert!(
        speedup >= 1.5,
        "Cranelift JIT speedup {speedup:.2}x is below the required 1.5x"
    );
}
