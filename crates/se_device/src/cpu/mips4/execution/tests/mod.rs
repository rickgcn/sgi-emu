//! R5000 instruction-level consistency tests.

mod cp0;
mod cp1;
mod functional;
mod integer;
mod memory;

use se_float::backend::softfloat3::SoftFloat3Backend;

use crate::cpu::execution::functional::FunctionalExecutor;
use crate::cpu::execution::protocol::{ExecutionAction, ExecutionCompletion, ExecutionTransaction};
use crate::cpu::mips4::config::{Mips4CacheConfig, Mips4Endianness};
use crate::cpu::mips4::execution::bus::{
    Mips4ExecutionAccessKind, Mips4ExecutionCompletion, Mips4ExecutionTransaction,
    Mips4ExecutionTransferSize,
};
use crate::cpu::mips4::execution::state::Mips4ExecutionState;
use crate::cpu::mips4::execution::target::{Mips4ExecutionBoundary, Mips4ExecutionTarget};
use crate::cpu::mips4::gpr::Mips4GprIndex;
use crate::cpu::mips4::model::r5000::boot_mode::R5000BootMode;
use crate::cpu::mips4::model::r5000::execution_policy::R5000ExecutionPolicy;
use crate::cpu::mips4::model::r5000::profile::R5000Profile;
use crate::cpu::mips4::model::r5000::revision::R5000Revision;

const RESET_PC: u64 = 0xffff_ffff_bfc0_0000;
const RESET_PHYSICAL_PC: u64 = 0x1fc0_0000;

type Executor = FunctionalExecutor<Mips4ExecutionTarget<R5000ExecutionPolicy, SoftFloat3Backend>>;

struct ConformanceMachine {
    executor: Executor,
    endianness: Mips4Endianness,
}

impl ConformanceMachine {
    fn new(endianness: Mips4Endianness) -> Self {
        Self::with_secondary(endianness, false)
    }

    fn with_secondary(endianness: Mips4Endianness, secondary: bool) -> Self {
        let secondary_cache = if secondary {
            Mips4CacheConfig::present(512 * 1024, 32)
        } else {
            Mips4CacheConfig::disabled()
        };
        let profile = R5000Profile::new(
            endianness,
            R5000Revision::from_bits(0x21),
            200_000_000,
            Mips4CacheConfig::present(32 * 1024, 32),
            Mips4CacheConfig::present(32 * 1024, 32),
            secondary_cache,
        );
        let boot_bits = if secondary { 1 << 12 } else { 0 };
        let policy =
            R5000ExecutionPolicy::new(profile, R5000BootMode::from_low_bits(boot_bits).unwrap());
        policy.validate_cache_config().unwrap();
        let target = Mips4ExecutionTarget::new(policy, SoftFloat3Backend::new()).unwrap();
        Self {
            executor: FunctionalExecutor::new(target),
            endianness,
        }
    }

    fn state(&self) -> &Mips4ExecutionState {
        self.executor.target().state()
    }

    fn state_mut(&mut self) -> &mut Mips4ExecutionState {
        self.executor.target_mut().state_mut()
    }

    fn write_gpr(&mut self, register: u8, value: u64) {
        self.state_mut()
            .gpr
            .write(Mips4GprIndex::from_u8(register).unwrap(), value);
    }

    fn read_gpr(&self, register: u8) -> u64 {
        self.state()
            .gpr()
            .read(Mips4GprIndex::from_u8(register).unwrap())
    }

    fn begin(
        &mut self,
        bits: u32,
    ) -> ExecutionAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary> {
        let ExecutionAction::Transaction(fetch) = self.executor.poll().unwrap() else {
            panic!("expected instruction fetch for {bits:#010x}");
        };
        assert_eq!(
            fetch.payload,
            Mips4ExecutionTransaction::Read {
                physical_address: RESET_PHYSICAL_PC + (self.state().pc() & 0x0fff),
                size: Mips4ExecutionTransferSize::Word,
                kind: Mips4ExecutionAccessKind::InstructionFetch,
                access_type: crate::cpu::mips4::cache::Mips4MemoryAccessType::Uncached,
            },
            "instruction fetch for {bits:#010x}"
        );
        self.executor
            .complete(ExecutionCompletion {
                id: fetch.id,
                payload: Mips4ExecutionCompletion::ReadData(self.word_lanes(bits)),
            })
            .unwrap();
        self.executor.poll().unwrap()
    }

    fn execute(&mut self, bits: u32) -> Mips4ExecutionBoundary {
        let ExecutionAction::Boundary(boundary) = self.begin(bits) else {
            panic!("expected instruction boundary for {bits:#010x}");
        };
        boundary
    }

    fn complete(
        &mut self,
        transaction: ExecutionTransaction<Mips4ExecutionTransaction>,
        completion: Mips4ExecutionCompletion,
    ) -> ExecutionAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary> {
        self.executor
            .complete(ExecutionCompletion {
                id: transaction.id,
                payload: completion,
            })
            .unwrap();
        self.executor.poll().unwrap()
    }

    fn execute_with_zero_bus(&mut self, bits: u32) -> Mips4ExecutionBoundary {
        let mut action = self.begin(bits);
        loop {
            match action {
                ExecutionAction::Boundary(boundary) => return boundary,
                ExecutionAction::Transaction(transaction) => {
                    let completion = match transaction.payload {
                        Mips4ExecutionTransaction::Read { .. } => {
                            Mips4ExecutionCompletion::ReadData(0)
                        }
                        Mips4ExecutionTransaction::Write { .. } => {
                            Mips4ExecutionCompletion::WriteComplete
                        }
                    };
                    action = self.complete(transaction, completion);
                }
                ExecutionAction::Waiting { .. } | ExecutionAction::Idle => {
                    panic!("instruction {bits:#010x} did not reach a boundary")
                }
            }
        }
    }

    fn word_lanes(&self, bits: u32) -> u64 {
        match self.endianness {
            Mips4Endianness::Big => {
                let bytes = bits.to_be_bytes();
                u64::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3], 0, 0, 0, 0])
            }
            Mips4Endianness::Little => u64::from(bits),
        }
    }
}

const fn r_type(rs: u8, rt: u8, rd: u8, shift: u8, function: u8) -> u32 {
    ((rs as u32) << 21)
        | ((rt as u32) << 16)
        | ((rd as u32) << 11)
        | ((shift as u32) << 6)
        | function as u32
}

const fn i_type(opcode: u8, rs: u8, rt: u8, immediate: u16) -> u32 {
    ((opcode as u32) << 26) | ((rs as u32) << 21) | ((rt as u32) << 16) | immediate as u32
}

const fn regimm(rs: u8, rt: u8, immediate: u16) -> u32 {
    i_type(0x01, rs, rt, immediate)
}

fn assert_retired(boundary: Mips4ExecutionBoundary, bits: u32) {
    assert_eq!(
        boundary,
        Mips4ExecutionBoundary::Retired {
            pc: RESET_PC,
            instruction: bits,
        }
    );
}
