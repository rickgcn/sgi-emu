//! Functional MIPS IV execution target.

use core::fmt;

use se_float::backend::FloatBackend;

use crate::cpu::execution::target::{
    ExecutionBoundary, ExecutionTarget, ExecutionTargetAction, ExecutionTargetSignalAction,
};
use crate::cpu::mips4::branch::Mips4BranchDecision;
use crate::cpu::mips4::cache::Mips4CacheInstruction;
use crate::cpu::mips4::cache::Mips4MemoryAccessType;
use crate::cpu::mips4::cache::hierarchy::{
    MIPS4_FUNCTIONAL_CACHE_LINE_BYTES, Mips4CacheAccessPolicy, Mips4CacheLine,
    Mips4InstructionCacheHit, line_base,
};
use crate::cpu::mips4::config::Mips4Endianness;
use crate::cpu::mips4::cp0::{Mips4Cp0CacheErr, Mips4Cp0Register};
use crate::cpu::mips4::cp1::decode::{
    Mips4Cp1Decode, decode_instruction as decode_cp1_instruction,
};
use crate::cpu::mips4::exception::{
    Mips4CoprocessorNumber, Mips4ErrorException, Mips4ErrorExceptionImage, Mips4Exception,
    Mips4ExceptionImage, Mips4ExceptionRestart,
};
use crate::cpu::mips4::gpr::{MIPS4_GPR_COUNT, Mips4GprIndex};
use crate::cpu::mips4::instruction::Mips4Instruction;
use crate::cpu::mips4::instruction::decode::{
    Mips4CpuInstruction, Mips4InstructionClass, Mips4InstructionDecode, decode_instruction,
};
use crate::cpu::mips4::instruction::requirements::{
    coprocessor_requirements, cp0_offset_requirements, cpu_requirements,
};
use crate::cpu::mips4::memory::ll_sc::Mips4LlBit;
use crate::cpu::mips4::memory::operation::{
    Mips4InstructionFetch, Mips4MemoryAccessError, Mips4Prefetch, Mips4PrefetchHint,
    Mips4PrefetchResult,
};
use crate::cpu::mips4::mmu::Mips4MmuPrivilegeMode;
use crate::cpu::mips4::tlb::{Mips4TlbAddressMode, Mips4TlbAsid};

use super::access::{Mips4InstructionAccess, check_architecture_level, check_coprocessor_access};
use super::block::{
    MIPS4_BLOCK_MAX_INSTRUCTIONS, Mips4Block, Mips4BlockBuildError, Mips4BlockFrame,
    Mips4BlockGuard, Mips4BlockGuardLine, Mips4BlockInstructionMetadata, Mips4BlockKey,
    Mips4BlockLiftedInstruction, Mips4BlockRuntime, Mips4CodeSourceRequest, Mips4CodeWindow,
    Mips4Cp0RuntimeOperation, Mips4FastMemoryReadRequest, Mips4FastMemoryReadResult,
    Mips4FastMemoryRuntime, Mips4RuntimeAbiV3, Mips4RuntimeOperation, Mips4RuntimeResult,
    lift_cpu_instruction,
};
use super::bus::{
    Mips4ExecutionAccessKind, Mips4ExecutionCompletion, Mips4ExecutionTransaction,
    Mips4ExecutionTransferSize,
};
use super::cp0::{Mips4Cp0Execution, check_cp0_access, decode_cp0_operation, execute_decoded_cp0};
use super::fpu::{Mips4FpuExecution, Mips4PendingFpuRead, complete_fpu_read, execute_fpu};
use super::integer::{Mips4CpuExecution, execute_cpu};
use super::memory::{
    Mips4MemoryPlan, Mips4PendingRead, Mips4PendingWrite, complete_read, complete_read_value,
    complete_write, complete_write_value, prepare_cache_address, prepare_memory,
    prepare_memory_with_operands,
};
use super::policy::{Mips4ExecutionPolicy, Mips4PrefetchPolicy};
use super::state::Mips4ExecutionConfigError;
use super::state::Mips4ExecutionState;

/// Asynchronous input accepted by functional MIPS IV execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Mips4ExecutionSignal {
    /// Replace external Cause IP line levels.
    ExternalInterrupts(u8),

    /// Clear an outstanding LL/SC reservation.
    InvalidateReservation,

    /// Raise a warm-reset exception and reset functional execution state machines.
    SoftReset,

    /// Latch a nonmaskable interrupt for the next instruction boundary.
    NonMaskableInterrupt,

    /// Raise a processor cache-error exception with a captured `CacheErr` value.
    CacheError(Mips4Cp0CacheErr),
}

/// Architectural boundary produced by functional MIPS IV execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Mips4ExecutionBoundary {
    /// One instruction committed normally.
    Retired {
        /// Address of the committed instruction.
        pc: u64,

        /// Raw committed instruction bits.
        instruction: u32,
    },

    /// An instruction or interrupt entered an architectural exception handler.
    Exception {
        /// Address at which the exception was observed.
        pc: u64,

        /// Captured architectural exception image.
        image: Mips4ExceptionImage,

        /// Selected exception vector.
        vector: u64,
    },

    /// A soft reset, NMI, or cache error entered CP0 error level.
    ErrorException {
        /// Address at which the error-level exception was observed.
        pc: u64,

        /// Captured error-level exception image.
        image: Mips4ErrorExceptionImage,

        /// Selected error-level exception vector.
        vector: u64,
    },
}

impl ExecutionBoundary for Mips4ExecutionBoundary {}

/// Internal functional MIPS IV target failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Mips4ExecutionTargetError {
    /// A completion arrived without a matching target-side operation.
    MissingPendingOperation,

    /// The bus completion kind did not match the pending operation.
    UnexpectedCompletion,

    /// More than one TLB entry matched an address.
    UndefinedMultipleTlbMatch,
}

impl fmt::Display for Mips4ExecutionTargetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingPendingOperation => write!(f, "no MIPS IV operation awaits completion"),
            Self::UnexpectedCompletion => write!(f, "unexpected MIPS IV bus completion kind"),
            Self::UndefinedMultipleTlbMatch => write!(f, "multiple MIPS IV TLB entries matched"),
        }
    }
}

impl std::error::Error for Mips4ExecutionTargetError {}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
enum PendingOperation {
    InstructionFetch {
        physical_address: u64,
    },
    DataRead {
        instruction: Mips4Instruction,
        pending: Mips4PendingRead,
    },
    DataWrite {
        instruction: Mips4Instruction,
        pending: Mips4PendingWrite,
    },
    FpuDataRead {
        instruction: Mips4Instruction,
        pending: Mips4PendingFpuRead,
    },
    FpuDataWrite {
        instruction: Mips4Instruction,
    },
    CacheRetire {
        instruction: Mips4Instruction,
    },
    Cached(Mips4PendingCachedAccess),
    CacheWriteback(Mips4PendingCacheWriteback),
}

#[derive(Clone, Copy, serde::Deserialize, serde::Serialize)]
struct PendingErrorException {
    reason: Mips4ErrorException,
    cache_error: Option<Mips4Cp0CacheErr>,
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
enum Mips4CachedClient {
    InstructionFetch {
        physical_address: u64,
    },
    DataRead {
        instruction: Mips4Instruction,
        pending: Mips4PendingRead,
    },
    DataWrite {
        instruction: Mips4Instruction,
        pending: Mips4PendingWrite,
    },
    FpuDataRead {
        instruction: Mips4Instruction,
        pending: Mips4PendingFpuRead,
    },
    FpuDataWrite {
        instruction: Mips4Instruction,
    },
    CacheRetire {
        instruction: Mips4Instruction,
    },
    Prefetch {
        instruction: Mips4Instruction,
    },
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
enum Mips4CachedStage {
    Writeback {
        doubleword: u8,
    },
    Fill {
        doubleword: u8,
        data: [u8; MIPS4_FUNCTIONAL_CACHE_LINE_BYTES],
    },
    WriteThrough,
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
struct Mips4PendingCachedAccess {
    client: Mips4CachedClient,
    virtual_address: u64,
    request: Mips4ExecutionTransaction,
    policy: Mips4CacheAccessPolicy,
    victim_set: usize,
    victim_way: usize,
    victim: Mips4CacheLine,
    secondary_source: Option<Mips4CacheLine>,
    use_secondary: bool,
    stage: Mips4CachedStage,
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
enum Mips4CacheWritebackTarget {
    PrimaryIndex {
        instruction_cache: bool,
        virtual_address: u64,
        invalidate: bool,
    },
    PrimaryHit {
        instruction_cache: bool,
        virtual_address: u64,
        physical_address: u64,
        invalidate: bool,
    },
    CreateDirty {
        virtual_address: u64,
        physical_address: u64,
        set: usize,
        way: usize,
    },
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
struct Mips4PendingCacheWriteback {
    instruction: Mips4Instruction,
    line: Mips4CacheLine,
    doubleword: u8,
    target: Mips4CacheWritebackTarget,
}

/// Functional MIPS IV execution target parameterized by processor policy and FPU backend.
#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct Mips4ExecutionTarget<P, F> {
    policy: P,
    float_backend: F,
    state: Mips4ExecutionState,
    pending: Option<PendingOperation>,
    pending_error_exception: Option<PendingErrorException>,
    #[serde(skip, default = "empty_decode_cache")]
    decode_cache: Box<[Option<DecodeCacheEntry>]>,
    #[serde(skip, default)]
    code_visibility: Mips4CodeVisibility,
    fetched_instruction: Option<Mips4FetchedInstruction>,
    #[serde(skip, default)]
    block_runtime_action:
        Option<ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary>>,
    #[serde(skip, default)]
    fast_memory_runtime: Option<Mips4RuntimeAbiV3>,
}

#[derive(Clone, Debug, Default)]
struct Mips4CodeVisibility {
    instruction_lines: Vec<Vec<u64>>,
    instruction_generation: u64,
    translation_generation: u64,
}

fn empty_decode_cache() -> Box<[Option<DecodeCacheEntry>]> {
    vec![None; 4096].into_boxed_slice()
}

#[derive(Clone, Copy, serde::Deserialize, serde::Serialize)]
struct DecodeCacheEntry {
    physical_address: u64,
    instruction: u32,
    decoded: Mips4InstructionDecode,
}

#[derive(Clone, Copy)]
struct Mips4CachedInstructionProbe {
    instruction: Mips4Instruction,
    hit: Mips4InstructionCacheHit,
}

#[derive(Clone, Copy, serde::Deserialize, serde::Serialize)]
struct Mips4FetchedInstruction {
    virtual_address: u64,
    physical_address: u64,
    instruction: Mips4Instruction,
}

const fn block_instruction_requires_counter_barrier(
    instruction: super::block::Mips4BlockInstruction,
) -> bool {
    matches!(
        instruction.operation,
        super::block::Mips4BlockOperation::Runtime(Mips4RuntimeOperation::Cp0 { .. })
    )
}

impl<P, F> Mips4ExecutionTarget<P, F>
where
    P: Mips4ExecutionPolicy,
    F: FloatBackend,
{
    /// Creates a functional target in reset state.
    pub fn new(policy: P, float_backend: F) -> Result<Self, Mips4ExecutionConfigError> {
        let state = Mips4ExecutionState::new(&policy)?;
        Ok(Self {
            policy,
            float_backend,
            state,
            pending: None,
            pending_error_exception: None,
            decode_cache: empty_decode_cache(),
            code_visibility: Mips4CodeVisibility::default(),
            fetched_instruction: None,
            block_runtime_action: None,
            fast_memory_runtime: None,
        })
    }

    /// Returns processor policy.
    pub const fn policy(&self) -> &P {
        &self.policy
    }

    /// Returns floating-point backend.
    pub const fn float_backend(&self) -> &F {
        &self.float_backend
    }

    /// Returns architectural state.
    pub const fn state(&self) -> &Mips4ExecutionState {
        &self.state
    }

    #[cfg(test)]
    pub(super) fn state_mut(&mut self) -> &mut Mips4ExecutionState {
        &mut self.state
    }

    /// Advances CP0 Count by externally calculated increments.
    pub fn advance_count(&mut self, count_increments: u64, timer_interrupt_enabled: bool) {
        self.state
            .cp0
            .advance_count(count_increments, timer_interrupt_enabled);
    }

    /// Advances CP0 Random after architectural instructions reach boundaries.
    pub fn advance_random(&mut self, instructions: u64) {
        self.state.cp0.advance_random(instructions);
    }

    /// Returns whether block execution may begin without bypassing a scalar boundary action.
    pub fn block_execution_ready(&self) -> bool {
        if self.pending.is_some() || self.state.standby {
            return false;
        }
        if self.dynamic_instruction_ready() {
            return true;
        }
        if self.pending_error_exception.is_some() {
            return false;
        }
        let status = self.state.cp0.status();
        let pending = self.state.cp0.cause().interrupt_pending() & status.interrupt_mask();
        !status.interrupts_enabled() || pending == 0
    }

    /// Returns the current block identity, including instruction-translation context.
    pub fn block_key(&self) -> Mips4BlockKey {
        let status = self.state.cp0.status();
        let config = self.state.cp0.config();
        let asid = self.state.cp0.entry_hi().address_space_identifier();
        let address = self.state.config.address;
        let fetch_context = u64::from(status.bits())
            ^ u64::from(config.bits()).rotate_left(32)
            ^ u64::from(asid).rotate_left(17)
            ^ u64::from(address.physical_address_bits).rotate_left(48)
            ^ u64::from(address.virtual_address_bits).rotate_left(56);
        Mips4BlockKey {
            pc: self.state.pc,
            next_pc: self.state.next_pc,
            delay_slot_branch_pc: self.state.delay_slot_branch_pc,
            fetch_context,
            translation_generation: self.code_visibility.translation_generation,
            code_guard: 0,
        }
    }

    /// Returns the current block identity with control state supplied by a hot frame.
    pub fn block_key_for_frame(&self, frame: &Mips4BlockFrame) -> Mips4BlockKey {
        let mut key = self.block_key();
        key.pc = frame.pc();
        key.next_pc = frame.next_pc();
        key.delay_slot_branch_pc = frame.delay_slot_branch_pc();
        key
    }

    /// Returns a side-effect-free code-source request for the current uncached PC.
    pub fn code_source_request(&self) -> Option<Mips4CodeSourceRequest> {
        if self.fetched_instruction.is_some() || self.pending.is_some() {
            return None;
        }
        self.code_source_request_at(self.state.pc)
    }

    fn code_source_request_at(&self, pc: u64) -> Option<Mips4CodeSourceRequest> {
        let status = self.state.cp0.status();
        let asid = Mips4TlbAsid::new(self.state.cp0.entry_hi().address_space_identifier());
        let tlb_entries = self.state.deterministic_tlb_entries(&self.policy, pc);
        let fetch = Mips4InstructionFetch::prepare(
            pc,
            self.policy.mmu_config(self.state.cp0.config()),
            status,
            asid,
            tlb_entries,
        )
        .ok()?;
        if self
            .policy
            .resolve_cache_policy(fetch.cache_attribute())
            .is_cached()
        {
            return None;
        }
        let page_remaining = 0x1000_u64 - (pc & 0x0fff);
        Some(Mips4CodeSourceRequest {
            virtual_address: pc,
            physical_address: fetch.physical_address(),
            maximum_bytes: page_remaining.min(128) as u8,
        })
    }

    /// Returns the block identity associated with one current code window.
    pub fn block_key_for_code_window(&self, window: &Mips4CodeWindow) -> Option<Mips4BlockKey> {
        self.block_key_for_code_window_control(
            window,
            self.state.pc,
            self.state.next_pc,
            self.state.delay_slot_branch_pc,
        )
    }

    /// Returns a stable-window block identity for hot frame control state.
    pub fn block_key_for_code_window_frame(
        &self,
        window: &Mips4CodeWindow,
        frame: &Mips4BlockFrame,
    ) -> Option<Mips4BlockKey> {
        self.block_key_for_code_window_control(
            window,
            frame.pc(),
            frame.next_pc(),
            frame.delay_slot_branch_pc(),
        )
    }

    fn block_key_for_code_window_control(
        &self,
        window: &Mips4CodeWindow,
        pc: u64,
        next_pc: u64,
        delay_slot_branch_pc: Option<u64>,
    ) -> Option<Mips4BlockKey> {
        let request = self.code_source_request_at(pc)?;
        window.instruction_index(request)?;
        let mut key = self.block_key();
        key.pc = pc;
        key.next_pc = next_pc;
        key.delay_slot_branch_pc = delay_slot_branch_pc;
        key.code_guard = window.guard().token();
        Some(key)
    }

    /// Checks that every instruction-cache line observed by a block is still visible.
    pub fn block_guard_valid(&self, guard: &Mips4BlockGuard) -> bool {
        guard.lines().iter().all(|guard_line| {
            let set = guard_line.set as usize;
            let way = usize::from(guard_line.way);
            let generation = self
                .code_visibility
                .instruction_lines
                .get(set)
                .and_then(|ways| ways.get(way))
                .copied()
                .unwrap_or(0);
            generation == guard_line.generation
        })
    }

    /// Builds a block only from side-effect-free hits in the modeled instruction cache.
    pub fn build_block(&self) -> Result<Option<Mips4Block>, Mips4BlockBuildError> {
        let key = self.block_key();
        let mut block = Mips4Block::new(key, Mips4BlockGuard::new());
        let mut pc = key.pc;

        if let Some(branch_pc) = key.delay_slot_branch_pc {
            let Some(probe) = self.probe_cached_instruction(pc) else {
                return Ok(None);
            };
            let metadata = Mips4BlockInstructionMetadata {
                pc,
                instruction: probe.instruction.bits(),
                delay_slot_branch_pc: Some(branch_pc),
            };
            let instruction = self.lift_cached_instruction(metadata, probe.instruction);
            let Mips4BlockLiftedInstruction::Sequential(instruction) = instruction else {
                return Ok(None);
            };
            block.add_guard_line(self.guard_line(probe.hit));
            block.push(instruction)?;
            block.terminate_dispatch()?;
            block.verify()?;
            return Ok(Some(block));
        }

        while block.instruction_count() < MIPS4_BLOCK_MAX_INSTRUCTIONS
            && pc & !0x0fff == key.pc & !0x0fff
        {
            let Some(probe) = self.probe_cached_instruction(pc) else {
                break;
            };
            let metadata = Mips4BlockInstructionMetadata {
                pc,
                instruction: probe.instruction.bits(),
                delay_slot_branch_pc: None,
            };
            let lifted = self.lift_cached_instruction(metadata, probe.instruction);
            match lifted {
                Mips4BlockLiftedInstruction::Sequential(instruction) => {
                    let counter_barrier = block_instruction_requires_counter_barrier(instruction);
                    if counter_barrier && block.instruction_count() != 0 {
                        break;
                    }
                    block.add_guard_line(self.guard_line(probe.hit));
                    block.push(instruction)?;
                    pc = pc.wrapping_add(4);
                    if counter_barrier {
                        break;
                    }
                }
                Mips4BlockLiftedInstruction::Branch(branch) => {
                    let delay_pc = pc.wrapping_add(4);
                    if block.instruction_count() + 2 > MIPS4_BLOCK_MAX_INSTRUCTIONS
                        || delay_pc & !0x0fff != key.pc & !0x0fff
                    {
                        break;
                    }
                    let Some(delay_probe) = self.probe_cached_instruction(delay_pc) else {
                        break;
                    };
                    let delay_metadata = Mips4BlockInstructionMetadata {
                        pc: delay_pc,
                        instruction: delay_probe.instruction.bits(),
                        delay_slot_branch_pc: Some(pc),
                    };
                    let Mips4BlockLiftedInstruction::Sequential(delay_slot) =
                        self.lift_cached_instruction(delay_metadata, delay_probe.instruction)
                    else {
                        break;
                    };
                    block.add_guard_line(self.guard_line(probe.hit));
                    if block_instruction_requires_counter_barrier(delay_slot) {
                        block.terminate_at_branch(branch)?;
                    } else {
                        block.add_guard_line(self.guard_line(delay_probe.hit));
                        block.terminate_with_branch(branch, delay_slot)?;
                    }
                    break;
                }
            }
        }

        if block.instruction_count() == 0 {
            Ok(None)
        } else {
            if block.branch().is_none() {
                block.terminate_dispatch()?;
            }
            block.verify()?;
            Ok(Some(block))
        }
    }

    /// Builds a block from a validated stable external code window.
    pub fn build_code_window(
        &self,
        window: &Mips4CodeWindow,
    ) -> Result<Option<Mips4Block>, Mips4BlockBuildError> {
        let Some(key) = self.block_key_for_code_window(window) else {
            return Ok(None);
        };
        self.build_code_window_for_key(window, key)
    }

    /// Builds a block at one validated control entry inside a stable code window.
    pub fn build_code_window_for_key(
        &self,
        window: &Mips4CodeWindow,
        key: Mips4BlockKey,
    ) -> Result<Option<Mips4Block>, Mips4BlockBuildError> {
        if self.block_key_for_code_window_control(
            window,
            key.pc,
            key.next_pc,
            key.delay_slot_branch_pc,
        ) != Some(key)
        {
            return Ok(None);
        }
        let Some(request) = self.code_source_request_at(key.pc) else {
            return Ok(None);
        };
        let Some(start_index) = window.instruction_index(request) else {
            return Ok(None);
        };
        let mut block = Mips4Block::new(key, Mips4BlockGuard::from_code_source(window.guard()));
        let decode_word = |bytes: &[u8]| match self.effective_endianness() {
            Mips4Endianness::Big => u32::from_be_bytes(bytes.try_into().unwrap()),
            Mips4Endianness::Little => u32::from_le_bytes(bytes.try_into().unwrap()),
        };
        let instruction_at = |index: usize| {
            let index = start_index + index;
            Mips4Instruction::from_bits(decode_word(&window.bytes()[index * 4..index * 4 + 4]))
        };

        if let Some(branch_pc) = key.delay_slot_branch_pc {
            let instruction = instruction_at(0);
            let metadata = Mips4BlockInstructionMetadata {
                pc: key.pc,
                instruction: instruction.bits(),
                delay_slot_branch_pc: Some(branch_pc),
            };
            match self.lift_cached_instruction(metadata, instruction) {
                Mips4BlockLiftedInstruction::Sequential(instruction) => {
                    block.push(instruction)?;
                    block.terminate_dispatch()?;
                }
                Mips4BlockLiftedInstruction::Branch(branch) => {
                    block.terminate_at_branch(branch)?;
                }
            }
            block.verify()?;
            return Ok(Some(block));
        }

        let available = window.bytes().len() / 4 - start_index;
        let mut index = 0;
        while index < available && block.instruction_count() < MIPS4_BLOCK_MAX_INSTRUCTIONS {
            let pc = key.pc.wrapping_add((index * 4) as u64);
            let instruction = instruction_at(index);
            let metadata = Mips4BlockInstructionMetadata {
                pc,
                instruction: instruction.bits(),
                delay_slot_branch_pc: None,
            };
            match self.lift_cached_instruction(metadata, instruction) {
                Mips4BlockLiftedInstruction::Sequential(instruction) => {
                    let counter_barrier = block_instruction_requires_counter_barrier(instruction);
                    if counter_barrier && block.instruction_count() != 0 {
                        break;
                    }
                    block.push(instruction)?;
                    index += 1;
                    if counter_barrier {
                        break;
                    }
                }
                Mips4BlockLiftedInstruction::Branch(branch) => {
                    if index + 1 < available
                        && block.instruction_count() + 2 <= MIPS4_BLOCK_MAX_INSTRUCTIONS
                    {
                        let delay = instruction_at(index + 1);
                        let delay_metadata = Mips4BlockInstructionMetadata {
                            pc: pc.wrapping_add(4),
                            instruction: delay.bits(),
                            delay_slot_branch_pc: Some(pc),
                        };
                        match self.lift_cached_instruction(delay_metadata, delay) {
                            Mips4BlockLiftedInstruction::Sequential(delay) => {
                                if block_instruction_requires_counter_barrier(delay) {
                                    block.terminate_at_branch(branch)?;
                                } else {
                                    block.terminate_with_branch(branch, delay)?;
                                }
                            }
                            Mips4BlockLiftedInstruction::Branch(_) => {
                                block.terminate_at_branch(branch)?;
                            }
                        }
                    } else {
                        block.terminate_at_branch(branch)?;
                    }
                    break;
                }
            }
        }
        if block.instruction_count() == 0 {
            return Ok(None);
        }
        if block.branch().is_none() {
            block.terminate_dispatch()?;
        }
        block.verify()?;
        Ok(Some(block))
    }

    /// Returns whether a completed instruction fetch is ready for the current PC.
    pub fn dynamic_instruction_ready(&self) -> bool {
        self.fetched_instruction
            .is_some_and(|fetched| fetched.virtual_address == self.state.pc)
    }

    /// Discards a completed fetch after a newly filled I-cache line became visible.
    pub fn discard_dynamic_instruction(&mut self) {
        self.fetched_instruction = None;
    }

    /// Builds one exhaustive block from a completed architectural fetch.
    pub fn take_dynamic_block(&mut self) -> Result<Option<Mips4Block>, Mips4BlockBuildError> {
        let Some(fetched) = self.fetched_instruction.take() else {
            return Ok(None);
        };
        if fetched.virtual_address != self.state.pc {
            return Ok(None);
        }
        let key = self.block_key();
        let metadata = Mips4BlockInstructionMetadata {
            pc: key.pc,
            instruction: fetched.instruction.bits(),
            delay_slot_branch_pc: key.delay_slot_branch_pc,
        };
        let lifted = self.lift_cached_instruction(metadata, fetched.instruction);
        let mut block = Mips4Block::new(key, Mips4BlockGuard::new());
        match lifted {
            Mips4BlockLiftedInstruction::Sequential(instruction) => {
                block.push(instruction)?;
                block.terminate_dispatch()?;
            }
            Mips4BlockLiftedInstruction::Branch(branch) => {
                block.terminate_at_branch(branch)?;
            }
        }
        block.verify()?;
        Ok(Some(block))
    }

    /// Starts a real architectural fetch for the block dispatcher.
    pub fn begin_block_fetch(
        &mut self,
    ) -> Result<
        ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary>,
        Mips4ExecutionTargetError,
    > {
        self.begin_fetch()
    }

    /// Copies integer and control state into the stable block ABI frame.
    pub fn block_frame(&self, budget: u64) -> Mips4BlockFrame {
        let mut gpr = [0; MIPS4_GPR_COUNT];
        for register in 1..MIPS4_GPR_COUNT as u8 {
            gpr[usize::from(register)] = self
                .state
                .gpr
                .read(Mips4GprIndex::from_u8(register).unwrap());
        }
        Mips4BlockFrame::new(
            gpr,
            self.state.hi,
            self.state.lo,
            self.state.pc,
            self.state.next_pc,
            self.state.delay_slot_branch_pc,
            budget,
        )
    }

    /// Binds block GPR writes directly to the architectural register file.
    pub fn bind_block_frame(&mut self, frame: &mut Mips4BlockFrame) {
        frame.bind_gpr_write_through(self.state.gpr.write_through_context());
    }

    /// Commits integer and control state produced by a block invocation.
    pub fn commit_block_frame(&mut self, frame: &mut Mips4BlockFrame) {
        let gpr_context = self.state.gpr.write_through_context();
        if frame.gpr_write_through() != gpr_context {
            for register in 1..MIPS4_GPR_COUNT as u8 {
                self.state.gpr.write(
                    Mips4GprIndex::from_u8(register).unwrap(),
                    frame.read_gpr(register),
                );
            }
        }
        frame.clear_gpr_write_through();
        self.state.hi = frame.hi();
        self.state.lo = frame.lo();
        self.state.pc = frame.pc();
        self.state.next_pc = frame.next_pc();
        self.state.delay_slot_branch_pc = frame.delay_slot_branch_pc();
    }

    /// Commits only control state needed to leave a cached block slice.
    pub fn commit_block_control(&mut self, frame: &Mips4BlockFrame) {
        self.state.pc = frame.pc();
        self.state.next_pc = frame.next_pc();
        self.state.delay_slot_branch_pc = frame.delay_slot_branch_pc();
    }

    /// Refreshes only frame control state changed by a target-side exception.
    pub fn refresh_block_control(&self, frame: &mut Mips4BlockFrame) {
        frame.replace_control(
            self.state.pc,
            self.state.next_pc,
            self.state.delay_slot_branch_pc,
        );
    }

    /// Refreshes a reusable frame after one protocol-completed boundary.
    pub fn refresh_block_boundary(
        &self,
        frame: &mut Mips4BlockFrame,
        boundary: &Mips4ExecutionBoundary,
    ) {
        self.refresh_block_control(frame);
        if let Mips4ExecutionBoundary::Retired { instruction, .. } = boundary {
            let register = Mips4Instruction::from_bits(*instruction).rt();
            frame.write_gpr(
                register,
                self.state
                    .gpr
                    .read(Mips4GprIndex::from_u8(register).unwrap()),
            );
        }
    }

    /// Takes an external action produced by one typed block runtime helper.
    pub fn take_block_runtime_action(
        &mut self,
    ) -> Option<ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary>> {
        self.block_runtime_action.take()
    }

    pub(crate) fn bind_fast_memory_runtime<R>(&mut self, runtime: &mut R)
    where
        R: Mips4FastMemoryRuntime,
    {
        debug_assert!(self.fast_memory_runtime.is_none());
        self.fast_memory_runtime = Some(Mips4RuntimeAbiV3::new(runtime));
    }

    pub(crate) fn clear_fast_memory_runtime(&mut self) {
        self.fast_memory_runtime = None;
    }

    fn prepare_runtime_frame(&mut self, frame: &Mips4BlockFrame, operation: Mips4RuntimeOperation) {
        self.state.pc = frame.pc();
        self.state.next_pc = frame.next_pc();
        self.state.delay_slot_branch_pc = frame.delay_slot_branch_pc();
        let registers = match operation {
            Mips4RuntimeOperation::Memory { .. } => [None, None],
            Mips4RuntimeOperation::Prefetch { raw }
            | Mips4RuntimeOperation::Cp0 { raw, .. }
            | Mips4RuntimeOperation::Cp1 { raw, .. } => [Some(raw.rs()), Some(raw.rt())],
            Mips4RuntimeOperation::Cache { base, .. } => [Some(base), None],
            Mips4RuntimeOperation::Coprocessor { .. } | Mips4RuntimeOperation::Raise(_) => {
                [None, None]
            }
        };
        for register in registers.into_iter().flatten() {
            self.state.gpr.write(
                Mips4GprIndex::from_u8(register).unwrap(),
                frame.read_gpr(register),
            );
        }
    }

    fn refresh_runtime_frame(&self, frame: &mut Mips4BlockFrame, operation: Mips4RuntimeOperation) {
        frame.replace_control(
            self.state.pc,
            self.state.next_pc,
            self.state.delay_slot_branch_pc,
        );
        let destination = match operation {
            Mips4RuntimeOperation::Memory {
                instruction:
                    Mips4CpuInstruction::Lb
                    | Mips4CpuInstruction::Lbu
                    | Mips4CpuInstruction::Lh
                    | Mips4CpuInstruction::Lhu
                    | Mips4CpuInstruction::Lw
                    | Mips4CpuInstruction::Lwu
                    | Mips4CpuInstruction::Ld
                    | Mips4CpuInstruction::Lwl
                    | Mips4CpuInstruction::Lwr
                    | Mips4CpuInstruction::Ldl
                    | Mips4CpuInstruction::Ldr
                    | Mips4CpuInstruction::Ll
                    | Mips4CpuInstruction::Lld
                    | Mips4CpuInstruction::Sc
                    | Mips4CpuInstruction::Scd,
                raw,
            } => Some(raw.rt()),
            Mips4RuntimeOperation::Memory { .. } | Mips4RuntimeOperation::Prefetch { .. } => None,
            Mips4RuntimeOperation::Cp0 { raw, .. } | Mips4RuntimeOperation::Cp1 { raw, .. } => {
                Some(raw.rt())
            }
            Mips4RuntimeOperation::Cache { .. }
            | Mips4RuntimeOperation::Coprocessor { .. }
            | Mips4RuntimeOperation::Raise(_) => None,
        };
        if let Some(register) = destination {
            frame.write_gpr(
                register,
                self.state
                    .gpr
                    .read(Mips4GprIndex::from_u8(register).unwrap()),
            );
        }
    }

    /// Enters an architectural exception recorded by block execution.
    pub fn finish_block_exception(
        &mut self,
        exception: super::block::Mips4BlockException,
    ) -> Mips4ExecutionBoundary {
        let ExecutionTargetAction::Boundary(boundary) =
            self.exception_boundary(exception.architecture_exception(), None)
        else {
            unreachable!();
        };
        boundary
    }

    fn probe_cached_instruction(&self, pc: u64) -> Option<Mips4CachedInstructionProbe> {
        let status = self.state.cp0.status();
        let asid = Mips4TlbAsid::new(self.state.cp0.entry_hi().address_space_identifier());
        let tlb_entries = self.state.deterministic_tlb_entries(&self.policy, pc);
        let fetch = Mips4InstructionFetch::prepare(
            pc,
            self.policy.mmu_config(self.state.cp0.config()),
            status,
            asid,
            tlb_entries,
        )
        .ok()?;
        let cache_policy = self.policy.resolve_cache_policy(fetch.cache_attribute());
        if !cache_policy.is_cached() || !self.state.cache.has_instruction() {
            return None;
        }
        let hit = self
            .state
            .cache
            .instruction_lookup_with_location(pc, fetch.physical_address())?;
        let (data_error, tag_error) = hit.line.check_errors(fetch.physical_address(), 4);
        if (data_error || tag_error) && !status.cache_error_disabled() {
            return None;
        }
        let lanes = hit.line.read_lanes(fetch.physical_address(), 4);
        Some(Mips4CachedInstructionProbe {
            instruction: Mips4Instruction::from_bits(self.instruction_word(lanes)),
            hit,
        })
    }

    fn lift_cached_instruction(
        &self,
        metadata: Mips4BlockInstructionMetadata,
        instruction: Mips4Instruction,
    ) -> Mips4BlockLiftedInstruction {
        let sequential_runtime = |operation| {
            Mips4BlockLiftedInstruction::Sequential(super::block::Mips4BlockInstruction {
                metadata,
                operation: super::block::Mips4BlockOperation::Runtime(operation),
                retire: super::block::Mips4BlockRetire { pc: metadata.pc },
            })
        };
        match decode_instruction(instruction) {
            Mips4InstructionDecode::Instruction(Mips4InstructionClass::Cpu(decoded)) => {
                match check_architecture_level(self.state.cp0.status(), cpu_requirements(decoded)) {
                    Mips4InstructionAccess::Execute => {
                        lift_cpu_instruction(&self.policy, metadata, decoded)
                    }
                    Mips4InstructionAccess::Exception(exception) => {
                        sequential_runtime(Mips4RuntimeOperation::Raise(exception))
                    }
                    Mips4InstructionAccess::FloatingPointUnimplemented => unreachable!(),
                }
            }
            Mips4InstructionDecode::Instruction(Mips4InstructionClass::Fpu(_)) => {
                let decoded = decode_cp1_instruction(instruction)
                    .unwrap_or(Mips4Cp1Decode::ReservedOrUnimplementedOperation);
                sequential_runtime(Mips4RuntimeOperation::Cp1 {
                    raw: instruction,
                    decoded,
                })
            }
            Mips4InstructionDecode::Instruction(Mips4InstructionClass::Coprocessor(
                Mips4CoprocessorNumber::Cp0,
            )) => sequential_runtime(Mips4RuntimeOperation::Cp0 {
                raw: instruction,
                operation: decode_cp0_operation(instruction),
            }),
            Mips4InstructionDecode::Instruction(Mips4InstructionClass::Coprocessor(
                coprocessor,
            )) => sequential_runtime(Mips4RuntimeOperation::Coprocessor {
                coprocessor,
                requirements: coprocessor_requirements(instruction),
            }),
            Mips4InstructionDecode::ReservedInstruction => sequential_runtime(
                Mips4RuntimeOperation::Raise(Mips4Exception::ReservedInstruction),
            ),
            Mips4InstructionDecode::UndefinedResult => {
                Mips4BlockLiftedInstruction::Sequential(super::block::Mips4BlockInstruction {
                    metadata,
                    operation: super::block::Mips4BlockOperation::NoOperation,
                    retire: super::block::Mips4BlockRetire { pc: metadata.pc },
                })
            }
            Mips4InstructionDecode::ProcessorSpecificCp0Offset => {
                let cache = Mips4CacheInstruction::from_instruction(instruction).unwrap();
                sequential_runtime(Mips4RuntimeOperation::Cache {
                    raw: instruction,
                    base: cache.base(),
                    offset: cache.offset(),
                    selector: cache.cache_selector_bits(),
                    operation: cache.operation_bits(),
                })
            }
        }
    }

    fn guard_line(&self, hit: Mips4InstructionCacheHit) -> Mips4BlockGuardLine {
        Mips4BlockGuardLine {
            set: hit.set as u32,
            way: hit.way as u8,
            physical_line_base: hit.line.physical_line_base,
            generation: self
                .code_visibility
                .instruction_lines
                .get(hit.set)
                .and_then(|ways| ways.get(hit.way))
                .copied()
                .unwrap_or(0),
        }
    }

    fn bump_instruction_line(&mut self, set: usize, way: usize) {
        self.code_visibility.instruction_generation =
            self.code_visibility.instruction_generation.wrapping_add(1);
        if self.code_visibility.instruction_lines.len() <= set {
            self.code_visibility
                .instruction_lines
                .resize_with(set + 1, Vec::new);
        }
        let ways = &mut self.code_visibility.instruction_lines[set];
        if ways.len() <= way {
            ways.resize(way + 1, 0);
        }
        let generation = &mut ways[way];
        *generation = generation.wrapping_add(1);
    }

    fn install_instruction_line(&mut self, set: usize, way: usize, line: Mips4CacheLine) {
        self.bump_instruction_line(set, way);
        self.state.cache.install_instruction(set, way, line);
    }

    fn primary_index_line_mut(
        &mut self,
        instruction_cache: bool,
        virtual_address: u64,
    ) -> Option<&mut Mips4CacheLine> {
        if instruction_cache {
            let (set, way) = self
                .state
                .cache
                .primary_index_location(true, virtual_address)?;
            self.bump_instruction_line(set, way);
        }
        self.state
            .cache
            .primary_index_line_mut(instruction_cache, virtual_address)
    }

    fn primary_hit_line_mut(
        &mut self,
        instruction_cache: bool,
        virtual_address: u64,
        physical_address: u64,
    ) -> Option<&mut Mips4CacheLine> {
        if instruction_cache {
            let (set, way) =
                self.state
                    .cache
                    .primary_hit_location(true, virtual_address, physical_address)?;
            self.bump_instruction_line(set, way);
        }
        self.state
            .cache
            .primary_hit_line_mut(instruction_cache, virtual_address, physical_address)
    }

    fn begin_fetch(
        &mut self,
    ) -> Result<
        ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary>,
        Mips4ExecutionTargetError,
    > {
        let status = self.state.cp0.status();
        let asid = Mips4TlbAsid::new(self.state.cp0.entry_hi().address_space_identifier());
        let tlb_entries = self
            .state
            .deterministic_tlb_entries(&self.policy, self.state.pc);
        match Mips4InstructionFetch::prepare(
            self.state.pc,
            self.policy.mmu_config(self.state.cp0.config()),
            status,
            asid,
            tlb_entries,
        ) {
            Ok(fetch) => {
                let access_type = self.policy.resolve_access_type(fetch.cache_attribute());
                let cache_policy = self.policy.resolve_cache_policy(fetch.cache_attribute());
                self.start_memory_access(
                    Mips4CachedClient::InstructionFetch {
                        physical_address: fetch.physical_address(),
                    },
                    self.state.pc,
                    cache_policy,
                    Mips4ExecutionTransaction::Read {
                        physical_address: fetch.physical_address(),
                        size: Mips4ExecutionTransferSize::Word,
                        kind: Mips4ExecutionAccessKind::InstructionFetch,
                        access_type,
                    },
                )
            }
            Err(error) => self.memory_error_boundary(error),
        }
    }

    fn start_memory_access(
        &mut self,
        client: Mips4CachedClient,
        virtual_address: u64,
        cache_policy: Mips4CacheAccessPolicy,
        request: Mips4ExecutionTransaction,
    ) -> Result<
        ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary>,
        Mips4ExecutionTargetError,
    > {
        let instruction = matches!(
            client,
            Mips4CachedClient::InstructionFetch { .. } | Mips4CachedClient::CacheRetire { .. }
        );
        let (physical_address, size, is_write) = transaction_shape(request);
        let cache_exists = if instruction {
            self.state.cache.has_instruction()
        } else {
            self.state.cache.has_data()
        };
        if !cache_policy.is_cached() || !cache_exists {
            self.pending = Some(direct_pending(client));
            return Ok(ExecutionTargetAction::Transaction(request));
        }

        let hit = if instruction {
            self.state
                .cache
                .instruction_lookup(virtual_address, physical_address)
        } else {
            self.state
                .cache
                .data_lookup(virtual_address, physical_address)
        };
        if let Some(line) = hit {
            let (data_error, tag_error) = line.check_errors(physical_address, size);
            if (data_error || tag_error) && !self.state.cp0.status().cache_error_disabled() {
                self.cancel_pending_nmi();
                let cache_error = Mips4Cp0CacheErr::primary_cache_error(
                    !instruction,
                    data_error,
                    tag_error,
                    physical_address,
                    virtual_address,
                );
                return Ok(self.error_exception_boundary(PendingErrorException {
                    reason: Mips4ErrorException::CacheError,
                    cache_error: Some(cache_error),
                }));
            }
            if !is_write {
                let lanes = line.read_lanes(physical_address, size);
                return self
                    .finish_cached_client(client, Mips4ExecutionCompletion::ReadData(lanes));
            }
            if cache_policy.is_write_back() {
                write_request_to_data_cache(&mut self.state, virtual_address, request, true);
                return self.finish_cached_client(client, Mips4ExecutionCompletion::WriteComplete);
            }
            let pending = Mips4PendingCachedAccess {
                client,
                virtual_address,
                request,
                policy: cache_policy,
                victim_set: 0,
                victim_way: 0,
                victim: Mips4CacheLine::INVALID,
                secondary_source: None,
                use_secondary: false,
                stage: Mips4CachedStage::WriteThrough,
            };
            self.pending = Some(PendingOperation::Cached(pending));
            return Ok(ExecutionTargetAction::Transaction(request));
        }

        if is_write && !cache_policy.write_allocates() {
            self.pending = Some(direct_pending(client));
            return Ok(ExecutionTargetAction::Transaction(request));
        }

        let Some((victim_set, victim_way, victim)) = (if instruction {
            self.state.cache.choose_instruction_victim(virtual_address)
        } else {
            self.state.cache.choose_data_victim(virtual_address)
        }) else {
            self.pending = Some(direct_pending(client));
            return Ok(ExecutionTargetAction::Transaction(request));
        };
        let use_secondary = cache_policy.is_write_back()
            && self.state.cp0.config().secondary_cache_enabled()
            && self.state.cache.has_secondary();
        let secondary_source = use_secondary
            .then(|| self.state.cache.secondary_lookup(physical_address))
            .flatten();
        let stage = if victim.valid && victim.dirty {
            Mips4CachedStage::Writeback { doubleword: 0 }
        } else {
            Mips4CachedStage::Fill {
                doubleword: 0,
                data: [0; MIPS4_FUNCTIONAL_CACHE_LINE_BYTES],
            }
        };
        if victim.valid && victim.dirty {
            let (data_error, tag_error) =
                victim.check_errors(victim.physical_line_base, MIPS4_FUNCTIONAL_CACHE_LINE_BYTES);
            if (data_error || tag_error) && !self.state.cp0.status().cache_error_disabled() {
                self.cancel_pending_nmi();
                let cache_error = Mips4Cp0CacheErr::primary_cache_error(
                    !instruction,
                    data_error,
                    tag_error,
                    victim.physical_line_base,
                    virtual_address,
                );
                return Ok(self.error_exception_boundary(PendingErrorException {
                    reason: Mips4ErrorException::CacheError,
                    cache_error: Some(cache_error),
                }));
            }
        }
        let pending = Mips4PendingCachedAccess {
            client,
            virtual_address,
            request,
            policy: cache_policy,
            victim_set,
            victim_way,
            victim,
            secondary_source,
            use_secondary,
            stage,
        };
        if victim.valid && victim.dirty {
            let transaction = cache_writeback_transaction(&pending, 0);
            self.pending = Some(PendingOperation::Cached(pending));
            return Ok(ExecutionTargetAction::Transaction(transaction));
        }
        self.start_cached_fill(pending)
    }

    fn complete_block_cached_read(
        &mut self,
        frame: &mut Mips4BlockFrame,
        pending: Mips4PendingRead,
        virtual_address: u64,
        cache_policy: Mips4CacheAccessPolicy,
        request: Mips4ExecutionTransaction,
    ) -> Result<(), Mips4PendingRead> {
        if !cache_policy.is_cached() || !self.state.cache.has_data() {
            return Err(pending);
        }
        let (physical_address, size, is_write) = transaction_shape(request);
        debug_assert!(!is_write);
        let Some(line) = self
            .state
            .cache
            .data_lookup(virtual_address, physical_address)
        else {
            return Err(pending);
        };
        if !self.state.cp0.status().cache_error_disabled() {
            let (data_error, tag_error) = line.check_errors(physical_address, size);
            if data_error || tag_error {
                return Err(pending);
            }
        }
        let lanes = line.read_lanes(physical_address, size);
        let endianness = self.effective_endianness();
        let (register, value) = complete_read_value(&mut self.state, pending, lanes, endianness);
        frame.write_gpr(register, value);
        retire_block_frame_control(frame);
        Ok(())
    }

    fn complete_block_cached_write(
        &mut self,
        frame: &mut Mips4BlockFrame,
        pending: Mips4PendingWrite,
        virtual_address: u64,
        cache_policy: Mips4CacheAccessPolicy,
        request: Mips4ExecutionTransaction,
    ) -> Result<(), Mips4PendingWrite> {
        if !cache_policy.is_cached()
            || !cache_policy.is_write_back()
            || !self.state.cache.has_data()
        {
            return Err(pending);
        }
        let (physical_address, size, is_write) = transaction_shape(request);
        debug_assert!(is_write);
        let Some(line) = self
            .state
            .cache
            .data_lookup(virtual_address, physical_address)
        else {
            return Err(pending);
        };
        if !self.state.cp0.status().cache_error_disabled() {
            let (data_error, tag_error) = line.check_errors(physical_address, size);
            if data_error || tag_error {
                return Err(pending);
            }
        }
        write_request_to_data_cache(&mut self.state, virtual_address, request, true);
        if let Some((register, value)) = complete_write_value(&mut self.state, pending) {
            frame.write_gpr(register, value);
        }
        retire_block_frame_control(frame);
        Ok(())
    }

    fn start_cached_fill(
        &mut self,
        mut pending: Mips4PendingCachedAccess,
    ) -> Result<
        ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary>,
        Mips4ExecutionTargetError,
    > {
        if let Some(line) = pending.secondary_source.take() {
            self.install_primary_line(&pending, line);
            return self.complete_cached_line(pending, line);
        }
        pending.stage = Mips4CachedStage::Fill {
            doubleword: 0,
            data: [0; MIPS4_FUNCTIONAL_CACHE_LINE_BYTES],
        };
        let transaction = cache_fill_transaction(&pending, 0);
        self.pending = Some(PendingOperation::Cached(pending));
        Ok(ExecutionTargetAction::Transaction(transaction))
    }

    fn install_primary_line(&mut self, pending: &Mips4PendingCachedAccess, line: Mips4CacheLine) {
        if matches!(
            pending.client,
            Mips4CachedClient::InstructionFetch { .. } | Mips4CachedClient::CacheRetire { .. }
        ) {
            self.install_instruction_line(pending.victim_set, pending.victim_way, line);
        } else {
            self.state
                .cache
                .install_data(pending.victim_set, pending.victim_way, line);
        }
    }

    fn complete_cached_line(
        &mut self,
        mut pending: Mips4PendingCachedAccess,
        line: Mips4CacheLine,
    ) -> Result<
        ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary>,
        Mips4ExecutionTargetError,
    > {
        let (physical_address, size, is_write) = transaction_shape(pending.request);
        if !is_write {
            return self.finish_cached_client(
                pending.client,
                Mips4ExecutionCompletion::ReadData(line.read_lanes(physical_address, size)),
            );
        }
        if pending.policy.is_write_back() {
            write_request_to_data_cache(
                &mut self.state,
                pending.virtual_address,
                pending.request,
                true,
            );
            return self
                .finish_cached_client(pending.client, Mips4ExecutionCompletion::WriteComplete);
        }
        pending.stage = Mips4CachedStage::WriteThrough;
        let request = pending.request;
        self.pending = Some(PendingOperation::Cached(pending));
        Ok(ExecutionTargetAction::Transaction(request))
    }

    fn finish_cached_client(
        &mut self,
        client: Mips4CachedClient,
        completion: Mips4ExecutionCompletion,
    ) -> Result<
        ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary>,
        Mips4ExecutionTargetError,
    > {
        match client {
            Mips4CachedClient::InstructionFetch { physical_address } => {
                self.complete_fetch(physical_address, completion)
            }
            Mips4CachedClient::DataRead {
                instruction,
                pending,
            } => self.complete_data_read(instruction, pending, completion),
            Mips4CachedClient::DataWrite {
                instruction,
                pending,
            } => self.complete_data_write(instruction, pending, completion),
            Mips4CachedClient::FpuDataRead {
                instruction,
                pending,
            } => self.complete_fpu_data_read(instruction, pending, completion),
            Mips4CachedClient::FpuDataWrite { instruction } => {
                self.complete_fpu_data_write(instruction, completion)
            }
            Mips4CachedClient::CacheRetire { instruction } => match completion {
                Mips4ExecutionCompletion::ReadData(_) | Mips4ExecutionCompletion::WriteComplete => {
                    Ok(self.retire_sequential(instruction))
                }
                Mips4ExecutionCompletion::BusError => {
                    Ok(self.exception_boundary(Mips4Exception::DataBusError, None))
                }
            },
            Mips4CachedClient::Prefetch { instruction } => Ok(self.retire_sequential(instruction)),
        }
    }

    fn complete_fetch(
        &mut self,
        physical_address: u64,
        completion: Mips4ExecutionCompletion,
    ) -> Result<
        ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary>,
        Mips4ExecutionTargetError,
    > {
        match completion {
            Mips4ExecutionCompletion::ReadData(data) => {
                let instruction = Mips4Instruction::from_bits(self.instruction_word(data));
                self.fetched_instruction = Some(Mips4FetchedInstruction {
                    virtual_address: self.state.pc,
                    physical_address,
                    instruction,
                });
                Ok(ExecutionTargetAction::Continue)
            }
            Mips4ExecutionCompletion::BusError => {
                Ok(self.exception_boundary(Mips4Exception::InstructionBusError, None))
            }
            Mips4ExecutionCompletion::WriteComplete => {
                Err(Mips4ExecutionTargetError::UnexpectedCompletion)
            }
        }
    }

    fn execute_instruction(
        &mut self,
        physical_address: u64,
        instruction: Mips4Instruction,
    ) -> Result<
        ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary>,
        Mips4ExecutionTargetError,
    > {
        let index = (physical_address as usize >> 2) & (self.decode_cache.len() - 1);
        let decoded = match self.decode_cache[index] {
            Some(entry)
                if entry.physical_address == physical_address
                    && entry.instruction == instruction.bits() =>
            {
                entry.decoded
            }
            _ => {
                let decoded = decode_instruction(instruction);
                self.decode_cache[index] = Some(DecodeCacheEntry {
                    physical_address,
                    instruction: instruction.bits(),
                    decoded,
                });
                decoded
            }
        };
        match decoded {
            Mips4InstructionDecode::Instruction(Mips4InstructionClass::Cpu(decoded)) => {
                if let Mips4InstructionAccess::Exception(exception) =
                    check_architecture_level(self.state.cp0.status(), cpu_requirements(decoded))
                {
                    return Ok(self.exception_boundary(exception, None));
                }
                if matches!(
                    decoded,
                    crate::cpu::mips4::instruction::decode::Mips4CpuInstruction::Pref
                ) {
                    return self.execute_prefetch(instruction);
                }
                match execute_cpu(&mut self.state, &self.policy, instruction, decoded) {
                    Mips4CpuExecution::Retire => Ok(self.retire_sequential(instruction)),
                    Mips4CpuExecution::Branch(decision) => {
                        Ok(self.retire_branch(instruction, decision))
                    }
                    Mips4CpuExecution::Memory(decoded) => {
                        match prepare_memory(
                            &self.state,
                            &self.policy,
                            instruction,
                            decoded,
                            self.effective_endianness(),
                        ) {
                            Ok(Mips4MemoryPlan::Read {
                                pending,
                                transaction,
                                virtual_address,
                                cache_policy,
                            }) => self.start_memory_access(
                                Mips4CachedClient::DataRead {
                                    instruction,
                                    pending,
                                },
                                virtual_address,
                                cache_policy,
                                transaction,
                            ),
                            Ok(Mips4MemoryPlan::Write {
                                pending,
                                transaction,
                                virtual_address,
                                cache_policy,
                            }) => self.start_memory_access(
                                Mips4CachedClient::DataWrite {
                                    instruction,
                                    pending,
                                },
                                virtual_address,
                                cache_policy,
                                transaction,
                            ),
                            Ok(Mips4MemoryPlan::Retire {
                                register_write,
                                clear_llbit,
                            }) => {
                                if let Some((register, value)) = register_write {
                                    self.state
                                        .gpr
                                        .write(Mips4GprIndex::from_u8(register).unwrap(), value);
                                }
                                if clear_llbit {
                                    self.state.llbit = Mips4LlBit::Clear;
                                }
                                Ok(self.retire_sequential(instruction))
                            }
                            Err(error) => self.memory_error_boundary(error),
                        }
                    }
                    Mips4CpuExecution::Exception(exception) => {
                        Ok(self.exception_boundary(exception, None))
                    }
                }
            }
            Mips4InstructionDecode::Instruction(Mips4InstructionClass::Coprocessor(
                Mips4CoprocessorNumber::Cp0,
            )) => {
                let operation = decode_cp0_operation(instruction);
                let result =
                    execute_decoded_cp0(&mut self.state, &self.policy, instruction, operation);
                Ok(self.finish_cp0(instruction, operation, result))
            }
            Mips4InstructionDecode::Instruction(Mips4InstructionClass::Coprocessor(
                coprocessor,
            )) => {
                let present = match coprocessor {
                    Mips4CoprocessorNumber::Cp2 => self.state.config.coprocessors.cp2,
                    Mips4CoprocessorNumber::Cp0
                    | Mips4CoprocessorNumber::Cp1
                    | Mips4CoprocessorNumber::Cp3 => false,
                };
                let access = check_coprocessor_access(
                    self.state.cp0.status(),
                    present,
                    coprocessor,
                    coprocessor_requirements(instruction),
                );
                let exception = match access {
                    Mips4InstructionAccess::Execute => Mips4Exception::ReservedInstruction,
                    Mips4InstructionAccess::Exception(exception) => exception,
                    Mips4InstructionAccess::FloatingPointUnimplemented => unreachable!(),
                };
                Ok(self.exception_boundary(exception, None))
            }
            Mips4InstructionDecode::Instruction(Mips4InstructionClass::Fpu(_)) => {
                let endianness = self.effective_endianness();
                let decoded = decode_cp1_instruction(instruction)
                    .unwrap_or(Mips4Cp1Decode::ReservedOrUnimplementedOperation);
                let result = execute_fpu(
                    &mut self.state,
                    &self.float_backend,
                    &self.policy,
                    instruction,
                    decoded,
                    endianness,
                );
                match result {
                    Ok(execution) => Ok(self.finish_fpu(instruction, execution)),
                    Err(error) => self.memory_error_boundary(error),
                }
            }
            Mips4InstructionDecode::ReservedInstruction => {
                Ok(self.exception_boundary(Mips4Exception::ReservedInstruction, None))
            }
            Mips4InstructionDecode::ProcessorSpecificCp0Offset => {
                self.execute_cache_instruction(instruction)
            }
            Mips4InstructionDecode::UndefinedResult => Ok(self.retire_sequential(instruction)),
        }
    }

    fn execute_cache_instruction(
        &mut self,
        instruction: Mips4Instruction,
    ) -> Result<
        ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary>,
        Mips4ExecutionTargetError,
    > {
        let cache = Mips4CacheInstruction::from_instruction(instruction).unwrap();
        self.execute_cache_operation(
            instruction,
            cache.base(),
            cache.offset(),
            cache.cache_selector_bits(),
            cache.operation_bits(),
        )
    }

    fn execute_cache_operation(
        &mut self,
        instruction: Mips4Instruction,
        base: u8,
        offset: i16,
        selector: u8,
        operation: u8,
    ) -> Result<
        ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary>,
        Mips4ExecutionTargetError,
    > {
        if let Err(exception) = check_cp0_access(self.state.cp0.status()) {
            return Ok(self.exception_boundary(exception, None));
        }
        if let Mips4InstructionAccess::Exception(exception) =
            check_architecture_level(self.state.cp0.status(), cp0_offset_requirements())
        {
            return Ok(self.exception_boundary(exception, None));
        }
        let address = match prepare_cache_address(&self.state, &self.policy, base, offset) {
            Ok(address) => address,
            Err(error) => return self.memory_error_boundary(error),
        };
        if !self
            .policy
            .resolve_cache_policy(address.cache_attribute)
            .is_cached()
        {
            return Ok(self.retire_sequential(instruction));
        }
        let virtual_address = address.virtual_address;
        let physical_address = address.physical_address;
        match (selector, operation) {
            (0, 0) => {
                if let Some(line) = self.primary_index_line_mut(true, virtual_address) {
                    line.valid = false;
                    line.dirty = false;
                }
            }
            (0, 1) => self.load_primary_tag(true, virtual_address),
            (0, 2) => self.store_primary_tag(true, virtual_address),
            (0, 4) => {
                if let Some(line) =
                    self.primary_hit_line_mut(true, virtual_address, physical_address)
                {
                    line.valid = false;
                    line.dirty = false;
                }
            }
            (0, 5) => {
                return self.start_instruction_cache_fill(
                    instruction,
                    virtual_address,
                    physical_address,
                );
            }
            (0, 6) => {
                let line = self.primary_hit_line_mut(true, virtual_address, physical_address);
                if let Some(line) = line.copied() {
                    return self.start_cache_writeback(
                        instruction,
                        line,
                        Mips4CacheWritebackTarget::PrimaryHit {
                            instruction_cache: true,
                            virtual_address,
                            physical_address,
                            invalidate: false,
                        },
                    );
                }
            }
            (1, 0) => {
                let line = self.state.cache.primary_index_line(false, virtual_address);
                if let Some(line) = line {
                    if line.valid && line.dirty {
                        return self.start_cache_writeback(
                            instruction,
                            line,
                            Mips4CacheWritebackTarget::PrimaryIndex {
                                instruction_cache: false,
                                virtual_address,
                                invalidate: true,
                            },
                        );
                    }
                    if let Some(line) = self.primary_index_line_mut(false, virtual_address) {
                        line.valid = false;
                        line.dirty = false;
                    }
                }
            }
            (1, 1) => self.load_primary_tag(false, virtual_address),
            (1, 2) => self.store_primary_tag(false, virtual_address),
            (1, 3) => {
                if let Some(line) =
                    self.primary_hit_line_mut(false, virtual_address, physical_address)
                {
                    line.dirty = true;
                } else if let Some((set, way, victim)) =
                    self.state.cache.choose_data_victim(virtual_address)
                {
                    if victim.valid && victim.dirty {
                        return self.start_cache_writeback(
                            instruction,
                            victim,
                            Mips4CacheWritebackTarget::CreateDirty {
                                virtual_address,
                                physical_address,
                                set,
                                way,
                            },
                        );
                    }
                    self.create_dirty_exclusive(
                        virtual_address,
                        physical_address,
                        set,
                        way,
                        victim,
                    );
                }
            }
            (1, 4) => {
                if let Some(line) =
                    self.primary_hit_line_mut(false, virtual_address, physical_address)
                {
                    line.valid = false;
                    line.dirty = false;
                }
            }
            (1, 5) | (1, 6) => {
                let invalidate = operation == 5;
                let line = self
                    .state
                    .cache
                    .data_lookup(virtual_address, physical_address);
                if let Some(line) = line {
                    if line.dirty {
                        return self.start_cache_writeback(
                            instruction,
                            line,
                            Mips4CacheWritebackTarget::PrimaryHit {
                                instruction_cache: false,
                                virtual_address,
                                physical_address,
                                invalidate,
                            },
                        );
                    }
                    if invalidate
                        && let Some(line) =
                            self.primary_hit_line_mut(false, virtual_address, physical_address)
                    {
                        line.valid = false;
                    }
                }
            }
            (3, 0) if self.state.cache.has_secondary() => {
                self.state.cache.secondary_flash_invalidate();
            }
            (3, 1) if self.state.cache.has_secondary() => {
                self.load_secondary_tag(physical_address);
            }
            (3, 2) if self.state.cache.has_secondary() => {
                self.store_secondary_tag(virtual_address, physical_address);
            }
            (3, 5) if self.state.cache.has_secondary() && virtual_address & 0x0fff == 0 => {
                self.state
                    .cache
                    .secondary_page_invalidate(physical_address & !0x0fff);
            }
            _ => {}
        }
        Ok(self.retire_sequential(instruction))
    }

    fn execute_prefetch(
        &mut self,
        instruction: Mips4Instruction,
    ) -> Result<
        ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary>,
        Mips4ExecutionTargetError,
    > {
        if matches!(
            self.policy.prefetch_policy(),
            Mips4PrefetchPolicy::NoOperation
        ) {
            return Ok(self.retire_sequential(instruction));
        }
        let hint = Mips4PrefetchHint::from_bits(instruction.rt());
        if !hint.is_defined() {
            return Ok(self.retire_sequential(instruction));
        }
        let base = self
            .state
            .gpr
            .read(Mips4GprIndex::from_u8(instruction.rs()).unwrap());
        let virtual_address = base.wrapping_add(instruction.signed_immediate() as i64 as u64);
        let tlb_entries = self
            .state
            .deterministic_tlb_entries(&self.policy, virtual_address);
        let result = Mips4Prefetch::prepare(
            virtual_address,
            hint,
            self.policy.mmu_config(self.state.cp0.config()),
            self.state.cp0.status(),
            Mips4TlbAsid::new(self.state.cp0.entry_hi().address_space_identifier()),
            tlb_entries,
        );
        let Mips4PrefetchResult::Request(prefetch) = result else {
            return Ok(self.retire_sequential(instruction));
        };
        let cache_policy = self.policy.resolve_cache_policy(prefetch.cache_attribute);
        if !cache_policy.is_cached() {
            return Ok(self.retire_sequential(instruction));
        }
        self.start_memory_access(
            Mips4CachedClient::Prefetch { instruction },
            prefetch.virtual_address,
            cache_policy,
            Mips4ExecutionTransaction::Read {
                physical_address: prefetch.physical_address,
                size: Mips4ExecutionTransferSize::Word,
                kind: Mips4ExecutionAccessKind::DataLoad,
                access_type: self.policy.resolve_access_type(prefetch.cache_attribute),
            },
        )
    }

    fn start_instruction_cache_fill(
        &mut self,
        instruction: Mips4Instruction,
        virtual_address: u64,
        physical_address: u64,
    ) -> Result<
        ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary>,
        Mips4ExecutionTargetError,
    > {
        let Some((victim_set, victim_way, victim)) =
            self.state.cache.choose_instruction_victim(virtual_address)
        else {
            return Ok(self.retire_sequential(instruction));
        };
        let use_secondary =
            self.state.cp0.config().secondary_cache_enabled() && self.state.cache.has_secondary();
        let secondary_source = use_secondary
            .then(|| self.state.cache.secondary_lookup(physical_address))
            .flatten();
        self.start_cached_fill(Mips4PendingCachedAccess {
            client: Mips4CachedClient::CacheRetire { instruction },
            virtual_address,
            request: Mips4ExecutionTransaction::Read {
                physical_address,
                size: Mips4ExecutionTransferSize::Word,
                kind: Mips4ExecutionAccessKind::InstructionFetch,
                access_type: Mips4MemoryAccessType::CachedNoncoherent,
            },
            policy: Mips4CacheAccessPolicy::WriteBackWriteAllocate,
            victim_set,
            victim_way,
            victim,
            secondary_source,
            use_secondary,
            stage: Mips4CachedStage::Fill {
                doubleword: 0,
                data: [0; MIPS4_FUNCTIONAL_CACHE_LINE_BYTES],
            },
        })
    }

    fn load_primary_tag(&mut self, instruction_cache: bool, virtual_address: u64) {
        let line = self
            .state
            .cache
            .primary_index_line(instruction_cache, virtual_address)
            .unwrap_or(Mips4CacheLine::INVALID);
        let mut tag_lo = (((line.physical_line_base >> 12) & 0x00ff_ffff) as u32) << 8;
        if line.tag_check_bit {
            tag_lo |= 1 << 7;
        }
        if line.valid {
            tag_lo |= 3 << 5;
        }
        let doubleword = ((virtual_address >> 3) & 3) as usize;
        let _ = self
            .state
            .cp0
            .write(Mips4Cp0Register::TagLo, u64::from(tag_lo));
        let _ = self.state.cp0.write(Mips4Cp0Register::TagHi, 0);
        let _ = self.state.cp0.write(
            Mips4Cp0Register::Ecc,
            u64::from(line.check_bits[doubleword]),
        );
    }

    fn store_primary_tag(&mut self, instruction_cache: bool, virtual_address: u64) {
        let tag_lo = self.state.cp0.tag_lo().bits();
        let ecc = self.state.cp0.ecc().bits() as u8;
        let check_override = self.state.cp0.status().cache_check_bits();
        if let Some(line) = self.primary_index_line_mut(instruction_cache, virtual_address) {
            line.physical_line_base = (u64::from((tag_lo >> 8) & 0x00ff_ffff) << 12)
                | (line_base(virtual_address) & 0x0fe0);
            line.valid = ((tag_lo >> 5) & 3) == 3;
            line.dirty = false;
            line.virtual_index = ((virtual_address >> 12) & 7) as u8;
            if check_override {
                line.tag_check_bit = tag_lo & (1 << 7) != 0;
                line.check_bits[((virtual_address >> 3) & 3) as usize] = ecc;
            } else {
                line.recompute_check_bits();
            }
        }
    }

    fn load_secondary_tag(&mut self, physical_address: u64) {
        let line = self
            .state
            .cache
            .secondary_index_line(physical_address)
            .unwrap_or(Mips4CacheLine::INVALID);
        let mut tag_lo = (((line.physical_line_base >> 19) & 0x1ffff) as u32) << 15;
        if line.valid {
            tag_lo |= 4 << 12;
        }
        let _ = self
            .state
            .cp0
            .write(Mips4Cp0Register::TagLo, u64::from(tag_lo));
        let _ = self.state.cp0.write(Mips4Cp0Register::TagHi, 0);
        let _ = self.state.cp0.write(
            Mips4Cp0Register::Ecc,
            u64::from(line.check_bits[((physical_address >> 3) & 3) as usize]),
        );
    }

    fn store_secondary_tag(&mut self, virtual_address: u64, physical_address: u64) {
        let tag_lo = self.state.cp0.tag_lo().bits();
        let ecc = self.state.cp0.ecc().bits() as u8;
        let check_override = self.state.cp0.status().cache_check_bits();
        if let Some(line) = self.state.cache.secondary_index_line_mut(physical_address) {
            line.physical_line_base = (u64::from((tag_lo >> 15) & 0x1ffff) << 19)
                | (line_base(physical_address) & 0x7_ffe0);
            line.valid = ((tag_lo >> 12) & 7) == 4;
            line.dirty = false;
            line.virtual_index = ((virtual_address >> 12) & 7) as u8;
            if check_override {
                line.check_bits[((physical_address >> 3) & 3) as usize] = ecc;
            } else {
                line.recompute_check_bits();
            }
        }
    }

    fn create_dirty_exclusive(
        &mut self,
        virtual_address: u64,
        physical_address: u64,
        set: usize,
        way: usize,
        mut line: Mips4CacheLine,
    ) {
        line.physical_line_base = line_base(physical_address);
        line.valid = true;
        line.dirty = true;
        line.virtual_index = ((virtual_address >> 12) & 7) as u8;
        line.recompute_check_bits();
        self.state.cache.install_data(set, way, line);
    }

    fn start_cache_writeback(
        &mut self,
        instruction: Mips4Instruction,
        line: Mips4CacheLine,
        target: Mips4CacheWritebackTarget,
    ) -> Result<
        ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary>,
        Mips4ExecutionTargetError,
    > {
        let pending = Mips4PendingCacheWriteback {
            instruction,
            line,
            doubleword: 0,
            target,
        };
        let transaction = cache_instruction_writeback_transaction(&pending);
        self.pending = Some(PendingOperation::CacheWriteback(pending));
        Ok(ExecutionTargetAction::Transaction(transaction))
    }

    fn finish_cp0(
        &mut self,
        instruction: Mips4Instruction,
        operation: Mips4Cp0RuntimeOperation,
        execution: Mips4Cp0Execution,
    ) -> ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary> {
        if matches!(execution, Mips4Cp0Execution::Retire)
            && matches!(
                operation,
                Mips4Cp0RuntimeOperation::TlbWriteIndexed
                    | Mips4Cp0RuntimeOperation::TlbWriteRandom
            )
        {
            self.code_visibility.translation_generation =
                self.code_visibility.translation_generation.wrapping_add(1);
        }
        match execution {
            Mips4Cp0Execution::Retire => self.retire_sequential(instruction),
            Mips4Cp0Execution::Standby => {
                self.state.standby = true;
                self.retire_sequential(instruction)
            }
            Mips4Cp0Execution::SetPc(pc) => self.retire_at_pc(instruction, pc),
            Mips4Cp0Execution::Exception(exception) => self.exception_boundary(exception, None),
        }
    }

    fn finish_fpu(
        &mut self,
        instruction: Mips4Instruction,
        execution: Mips4FpuExecution,
    ) -> ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary> {
        match execution {
            Mips4FpuExecution::Retire => self.retire_sequential(instruction),
            Mips4FpuExecution::Branch(decision) => self.retire_branch(instruction, decision),
            Mips4FpuExecution::Read {
                pending,
                transaction,
                virtual_address,
                cache_policy,
            } => self
                .start_memory_access(
                    Mips4CachedClient::FpuDataRead {
                        instruction,
                        pending,
                    },
                    virtual_address,
                    cache_policy,
                    transaction,
                )
                .expect("cache access setup cannot fail"),
            Mips4FpuExecution::Write {
                transaction,
                virtual_address,
                cache_policy,
            } => self
                .start_memory_access(
                    Mips4CachedClient::FpuDataWrite { instruction },
                    virtual_address,
                    cache_policy,
                    transaction,
                )
                .expect("cache access setup cannot fail"),
            Mips4FpuExecution::Prefetch(prefetch) => {
                let cache_policy = self.policy.resolve_cache_policy(prefetch.cache_attribute);
                if !cache_policy.is_cached() {
                    return self.retire_sequential(instruction);
                }
                self.start_memory_access(
                    Mips4CachedClient::Prefetch { instruction },
                    prefetch.virtual_address,
                    cache_policy,
                    Mips4ExecutionTransaction::Read {
                        physical_address: prefetch.physical_address,
                        size: Mips4ExecutionTransferSize::Word,
                        kind: Mips4ExecutionAccessKind::DataLoad,
                        access_type: self.policy.resolve_access_type(prefetch.cache_attribute),
                    },
                )
                .expect("cache access setup cannot fail")
            }
            Mips4FpuExecution::Exception(exception) => self.exception_boundary(exception, None),
        }
    }

    fn retire_sequential(
        &mut self,
        instruction: Mips4Instruction,
    ) -> ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary> {
        let pc = self.state.pc;
        self.state.pc = self.state.next_pc;
        self.state.next_pc = self.state.next_pc.wrapping_add(4);
        self.state.delay_slot_branch_pc = None;
        ExecutionTargetAction::Boundary(Mips4ExecutionBoundary::Retired {
            pc,
            instruction: instruction.bits(),
        })
    }

    fn retire_branch(
        &mut self,
        instruction: Mips4Instruction,
        decision: Mips4BranchDecision,
    ) -> ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary> {
        let pc = self.state.pc;
        if decision.nullify_delay_slot() {
            self.state.pc = pc.wrapping_add(8);
            self.state.next_pc = pc.wrapping_add(12);
            self.state.delay_slot_branch_pc = None;
        } else {
            self.state.pc = self.state.next_pc;
            self.state.next_pc = decision.target().unwrap_or_else(|| pc.wrapping_add(8));
            self.state.delay_slot_branch_pc = Some(pc);
        }

        ExecutionTargetAction::Boundary(Mips4ExecutionBoundary::Retired {
            pc,
            instruction: instruction.bits(),
        })
    }

    fn retire_at_pc(
        &mut self,
        instruction: Mips4Instruction,
        destination: u64,
    ) -> ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary> {
        let pc = self.state.pc;
        self.state.pc = destination;
        self.state.next_pc = destination.wrapping_add(4);
        self.state.delay_slot_branch_pc = None;
        ExecutionTargetAction::Boundary(Mips4ExecutionBoundary::Retired {
            pc,
            instruction: instruction.bits(),
        })
    }

    fn memory_error_boundary(
        &mut self,
        error: Mips4MemoryAccessError,
    ) -> Result<
        ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary>,
        Mips4ExecutionTargetError,
    > {
        match error {
            Mips4MemoryAccessError::AddressError {
                exception,
                virtual_address,
            } => Ok(self.exception_boundary(exception, Some(virtual_address))),
            Mips4MemoryAccessError::TranslationFault(fault) => Ok(self
                .exception_boundary_with_refill(
                    fault.exception,
                    Some(fault.bad_virtual_address),
                    self.refill_address_mode(fault.bad_virtual_address, fault.address_mode),
                )),
            Mips4MemoryAccessError::UndefinedMultipleTlbMatch { .. } => {
                Err(Mips4ExecutionTargetError::UndefinedMultipleTlbMatch)
            }
        }
    }

    fn exception_boundary(
        &mut self,
        reason: Mips4Exception,
        bad_virtual_address: Option<u64>,
    ) -> ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary> {
        if matches!(
            reason,
            Mips4Exception::InstructionBusError | Mips4Exception::DataBusError
        ) {
            self.cancel_pending_nmi();
        }
        self.exception_boundary_with_refill(reason, bad_virtual_address, None)
    }

    fn exception_boundary_with_refill(
        &mut self,
        reason: Mips4Exception,
        bad_virtual_address: Option<u64>,
        refill_address_mode: Option<Mips4TlbAddressMode>,
    ) -> ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary> {
        let pc = self.state.pc;
        let status = self.state.cp0.status();
        let restart = Mips4ExceptionRestart::new(pc, self.state.delay_slot_branch_pc);
        let image = Mips4ExceptionImage::new(reason, restart, bad_virtual_address);
        let vector = self
            .policy
            .exception_vector(status, image, refill_address_mode);
        self.state.cp0.enter_exception(image);
        self.state.pc = vector;
        self.state.next_pc = vector.wrapping_add(4);
        self.state.delay_slot_branch_pc = None;
        self.state.llbit = Mips4LlBit::Clear;
        ExecutionTargetAction::Boundary(Mips4ExecutionBoundary::Exception { pc, image, vector })
    }

    fn error_exception_boundary(
        &mut self,
        pending: PendingErrorException,
    ) -> ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary> {
        let pc = self.state.pc;
        let status = self.state.cp0.status();
        let restart = Mips4ExceptionRestart::new(pc, self.state.delay_slot_branch_pc);
        let image = match pending.cache_error {
            Some(cache_error) => Mips4ErrorExceptionImage::cache_error(restart, cache_error.bits()),
            None => Mips4ErrorExceptionImage::new(pending.reason, restart),
        };
        let vector = self.policy.error_exception_vector(status, pending.reason);
        self.state.cp0.enter_error_exception(image);
        self.state.pc = vector;
        self.state.next_pc = vector.wrapping_add(4);
        self.state.delay_slot_branch_pc = None;
        self.state.llbit = Mips4LlBit::Clear;
        self.state.standby = false;
        ExecutionTargetAction::Boundary(Mips4ExecutionBoundary::ErrorException {
            pc,
            image,
            vector,
        })
    }

    fn latch_error_exception(&mut self, pending: PendingErrorException) {
        let replace = match (pending.reason, self.pending_error_exception) {
            (Mips4ErrorException::SoftReset, _) => true,
            (
                Mips4ErrorException::CacheError,
                Some(PendingErrorException {
                    reason: Mips4ErrorException::SoftReset,
                    ..
                }),
            ) => false,
            (Mips4ErrorException::CacheError, _) => true,
            (Mips4ErrorException::NonMaskableInterrupt, None) => true,
            (Mips4ErrorException::NonMaskableInterrupt, Some(_)) => false,
        };
        if replace {
            self.pending_error_exception = Some(pending);
        }
    }

    fn cancel_pending_nmi(&mut self) {
        if matches!(
            self.pending_error_exception,
            Some(PendingErrorException {
                reason: Mips4ErrorException::NonMaskableInterrupt,
                ..
            })
        ) {
            self.pending_error_exception = None;
        }
    }

    fn refill_address_mode(
        &self,
        address: u64,
        address_mode: Option<Mips4TlbAddressMode>,
    ) -> Option<Mips4TlbAddressMode> {
        let address_mode = address_mode?;
        let asid = Mips4TlbAsid::new(self.state.cp0.entry_hi().address_space_identifier());
        let matched = self
            .state
            .tlb_entries
            .iter()
            .any(|entry| entry.matches_virtual_address(address, asid, address_mode));
        (!matched).then_some(address_mode)
    }

    fn instruction_word(&self, physical_lanes: u64) -> u32 {
        let bytes = physical_lanes.to_le_bytes();
        let encoded = [bytes[0], bytes[1], bytes[2], bytes[3]];
        match self.effective_endianness() {
            Mips4Endianness::Big => u32::from_be_bytes(encoded),
            Mips4Endianness::Little => u32::from_le_bytes(encoded),
        }
    }

    fn effective_endianness(&self) -> Mips4Endianness {
        let status = self.state.cp0.status();
        let reverse_endian = matches!(
            Mips4MmuPrivilegeMode::from_status(status),
            Some(Mips4MmuPrivilegeMode::User)
        ) && status.reverse_endianness();
        self.state
            .config
            .endianness
            .effective_cpu_endianness(reverse_endian)
    }

    fn complete_data_read(
        &mut self,
        instruction: Mips4Instruction,
        pending: Mips4PendingRead,
        completion: Mips4ExecutionCompletion,
    ) -> Result<
        ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary>,
        Mips4ExecutionTargetError,
    > {
        match completion {
            Mips4ExecutionCompletion::ReadData(data) => {
                let endianness = self.effective_endianness();
                complete_read(&mut self.state, pending, data, endianness);
                Ok(self.retire_sequential(instruction))
            }
            Mips4ExecutionCompletion::BusError => {
                Ok(self.exception_boundary(Mips4Exception::DataBusError, None))
            }
            Mips4ExecutionCompletion::WriteComplete => {
                Err(Mips4ExecutionTargetError::UnexpectedCompletion)
            }
        }
    }

    fn complete_data_write(
        &mut self,
        instruction: Mips4Instruction,
        pending: Mips4PendingWrite,
        completion: Mips4ExecutionCompletion,
    ) -> Result<
        ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary>,
        Mips4ExecutionTargetError,
    > {
        match completion {
            Mips4ExecutionCompletion::WriteComplete => {
                complete_write(&mut self.state, pending);
                Ok(self.retire_sequential(instruction))
            }
            Mips4ExecutionCompletion::BusError => {
                Ok(self.exception_boundary(Mips4Exception::DataBusError, None))
            }
            Mips4ExecutionCompletion::ReadData(_) => {
                Err(Mips4ExecutionTargetError::UnexpectedCompletion)
            }
        }
    }

    fn complete_fpu_data_read(
        &mut self,
        instruction: Mips4Instruction,
        pending: Mips4PendingFpuRead,
        completion: Mips4ExecutionCompletion,
    ) -> Result<
        ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary>,
        Mips4ExecutionTargetError,
    > {
        match completion {
            Mips4ExecutionCompletion::ReadData(data) => {
                let endianness = self.effective_endianness();
                match complete_fpu_read(&mut self.state, pending, data, endianness) {
                    Mips4FpuExecution::Retire => Ok(self.retire_sequential(instruction)),
                    Mips4FpuExecution::Exception(exception) => {
                        Ok(self.exception_boundary(exception, None))
                    }
                    Mips4FpuExecution::Branch(_)
                    | Mips4FpuExecution::Read { .. }
                    | Mips4FpuExecution::Write { .. }
                    | Mips4FpuExecution::Prefetch(_) => {
                        Err(Mips4ExecutionTargetError::UnexpectedCompletion)
                    }
                }
            }
            Mips4ExecutionCompletion::BusError => {
                Ok(self.exception_boundary(Mips4Exception::DataBusError, None))
            }
            Mips4ExecutionCompletion::WriteComplete => {
                Err(Mips4ExecutionTargetError::UnexpectedCompletion)
            }
        }
    }

    fn complete_fpu_data_write(
        &mut self,
        instruction: Mips4Instruction,
        completion: Mips4ExecutionCompletion,
    ) -> Result<
        ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary>,
        Mips4ExecutionTargetError,
    > {
        match completion {
            Mips4ExecutionCompletion::WriteComplete => Ok(self.retire_sequential(instruction)),
            Mips4ExecutionCompletion::BusError => {
                Ok(self.exception_boundary(Mips4Exception::DataBusError, None))
            }
            Mips4ExecutionCompletion::ReadData(_) => {
                Err(Mips4ExecutionTargetError::UnexpectedCompletion)
            }
        }
    }

    fn complete_cached_access(
        &mut self,
        mut pending: Mips4PendingCachedAccess,
        completion: Mips4ExecutionCompletion,
    ) -> Result<
        ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary>,
        Mips4ExecutionTargetError,
    > {
        if matches!(completion, Mips4ExecutionCompletion::BusError) {
            if matches!(pending.client, Mips4CachedClient::Prefetch { .. }) {
                return self.finish_cached_client(pending.client, completion);
            }
            let exception = if matches!(pending.client, Mips4CachedClient::InstructionFetch { .. })
            {
                Mips4Exception::InstructionBusError
            } else {
                Mips4Exception::DataBusError
            };
            return Ok(self.exception_boundary(exception, None));
        }
        match pending.stage {
            Mips4CachedStage::Writeback { doubleword } => {
                if !matches!(completion, Mips4ExecutionCompletion::WriteComplete) {
                    return Err(Mips4ExecutionTargetError::UnexpectedCompletion);
                }
                if doubleword < 3 {
                    let next = doubleword + 1;
                    pending.stage = Mips4CachedStage::Writeback { doubleword: next };
                    let transaction = cache_writeback_transaction(&pending, next);
                    self.pending = Some(PendingOperation::Cached(pending));
                    return Ok(ExecutionTargetAction::Transaction(transaction));
                }
                if pending.use_secondary {
                    self.state.cache.secondary_install(pending.victim);
                }
                self.start_cached_fill(pending)
            }
            Mips4CachedStage::Fill {
                doubleword,
                mut data,
            } => {
                let Mips4ExecutionCompletion::ReadData(lanes) = completion else {
                    return Err(Mips4ExecutionTargetError::UnexpectedCompletion);
                };
                let offset = usize::from(doubleword) * 8;
                data[offset..offset + 8].copy_from_slice(&lanes.to_le_bytes());
                if doubleword < 3 {
                    let next = doubleword + 1;
                    pending.stage = Mips4CachedStage::Fill {
                        doubleword: next,
                        data,
                    };
                    let transaction = cache_fill_transaction(&pending, next);
                    self.pending = Some(PendingOperation::Cached(pending));
                    return Ok(ExecutionTargetAction::Transaction(transaction));
                }
                let (physical_address, _, _) = transaction_shape(pending.request);
                let line = Mips4CacheLine::from_data(
                    line_base(physical_address),
                    pending.virtual_address,
                    data,
                );
                if pending.use_secondary {
                    self.state.cache.secondary_install(line);
                }
                self.install_primary_line(&pending, line);
                self.complete_cached_line(pending, line)
            }
            Mips4CachedStage::WriteThrough => {
                if !matches!(completion, Mips4ExecutionCompletion::WriteComplete) {
                    return Err(Mips4ExecutionTargetError::UnexpectedCompletion);
                }
                write_request_to_data_cache(
                    &mut self.state,
                    pending.virtual_address,
                    pending.request,
                    false,
                );
                self.finish_cached_client(pending.client, completion)
            }
        }
    }

    fn complete_cache_writeback(
        &mut self,
        mut pending: Mips4PendingCacheWriteback,
        completion: Mips4ExecutionCompletion,
    ) -> Result<
        ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary>,
        Mips4ExecutionTargetError,
    > {
        match completion {
            Mips4ExecutionCompletion::BusError => {
                return Ok(self.exception_boundary(Mips4Exception::DataBusError, None));
            }
            Mips4ExecutionCompletion::ReadData(_) => {
                return Err(Mips4ExecutionTargetError::UnexpectedCompletion);
            }
            Mips4ExecutionCompletion::WriteComplete => {}
        }
        if pending.doubleword < 3 {
            pending.doubleword += 1;
            let transaction = cache_instruction_writeback_transaction(&pending);
            self.pending = Some(PendingOperation::CacheWriteback(pending));
            return Ok(ExecutionTargetAction::Transaction(transaction));
        }
        if self.state.cache.has_secondary() {
            self.state.cache.secondary_install(pending.line);
        }
        match pending.target {
            Mips4CacheWritebackTarget::PrimaryIndex {
                instruction_cache,
                virtual_address,
                invalidate,
            } => {
                if let Some(line) = self.primary_index_line_mut(instruction_cache, virtual_address)
                {
                    line.dirty = false;
                    if invalidate {
                        line.valid = false;
                    }
                }
            }
            Mips4CacheWritebackTarget::PrimaryHit {
                instruction_cache,
                virtual_address,
                physical_address,
                invalidate,
            } => {
                if let Some(line) =
                    self.primary_hit_line_mut(instruction_cache, virtual_address, physical_address)
                {
                    line.dirty = false;
                    if invalidate {
                        line.valid = false;
                    }
                }
            }
            Mips4CacheWritebackTarget::CreateDirty {
                virtual_address,
                physical_address,
                set,
                way,
            } => self.create_dirty_exclusive(
                virtual_address,
                physical_address,
                set,
                way,
                pending.line,
            ),
        }
        Ok(self.retire_sequential(pending.instruction))
    }
}

fn direct_pending(client: Mips4CachedClient) -> PendingOperation {
    match client {
        Mips4CachedClient::InstructionFetch { physical_address } => {
            PendingOperation::InstructionFetch { physical_address }
        }
        Mips4CachedClient::DataRead {
            instruction,
            pending,
        } => PendingOperation::DataRead {
            instruction,
            pending,
        },
        Mips4CachedClient::DataWrite {
            instruction,
            pending,
        } => PendingOperation::DataWrite {
            instruction,
            pending,
        },
        Mips4CachedClient::FpuDataRead {
            instruction,
            pending,
        } => PendingOperation::FpuDataRead {
            instruction,
            pending,
        },
        Mips4CachedClient::FpuDataWrite { instruction } => {
            PendingOperation::FpuDataWrite { instruction }
        }
        Mips4CachedClient::CacheRetire { instruction } => {
            PendingOperation::CacheRetire { instruction }
        }
        Mips4CachedClient::Prefetch { instruction } => {
            PendingOperation::CacheRetire { instruction }
        }
    }
}

fn transaction_shape(transaction: Mips4ExecutionTransaction) -> (u64, usize, bool) {
    match transaction {
        Mips4ExecutionTransaction::Read {
            physical_address,
            size,
            ..
        } => (physical_address, usize::from(size.bytes()), false),
        Mips4ExecutionTransaction::Write {
            physical_address,
            size,
            ..
        } => (physical_address, usize::from(size.bytes()), true),
    }
}

fn cache_fill_transaction(
    pending: &Mips4PendingCachedAccess,
    doubleword: u8,
) -> Mips4ExecutionTransaction {
    let (physical_address, _, _) = transaction_shape(pending.request);
    let kind = if matches!(
        pending.client,
        Mips4CachedClient::InstructionFetch { .. } | Mips4CachedClient::CacheRetire { .. }
    ) {
        Mips4ExecutionAccessKind::InstructionFetch
    } else {
        Mips4ExecutionAccessKind::DataLoad
    };
    Mips4ExecutionTransaction::Read {
        physical_address: line_base(physical_address) + u64::from(doubleword) * 8,
        size: Mips4ExecutionTransferSize::Doubleword,
        kind,
        access_type: Mips4MemoryAccessType::CachedNoncoherent,
    }
}

fn cache_writeback_transaction(
    pending: &Mips4PendingCachedAccess,
    doubleword: u8,
) -> Mips4ExecutionTransaction {
    let offset = usize::from(doubleword) * 8;
    let data = u64::from_le_bytes(pending.victim.data[offset..offset + 8].try_into().unwrap());
    Mips4ExecutionTransaction::Write {
        physical_address: pending.victim.physical_line_base + u64::from(doubleword) * 8,
        size: Mips4ExecutionTransferSize::Doubleword,
        data,
        byte_enable: 0xff,
        access_type: Mips4MemoryAccessType::CachedNoncoherent,
    }
}

fn cache_instruction_writeback_transaction(
    pending: &Mips4PendingCacheWriteback,
) -> Mips4ExecutionTransaction {
    let offset = usize::from(pending.doubleword) * 8;
    Mips4ExecutionTransaction::Write {
        physical_address: pending.line.physical_line_base + u64::from(pending.doubleword) * 8,
        size: Mips4ExecutionTransferSize::Doubleword,
        data: u64::from_le_bytes(pending.line.data[offset..offset + 8].try_into().unwrap()),
        byte_enable: 0xff,
        access_type: Mips4MemoryAccessType::CachedNoncoherent,
    }
}

fn write_request_to_data_cache(
    state: &mut Mips4ExecutionState,
    virtual_address: u64,
    request: Mips4ExecutionTransaction,
    dirty: bool,
) {
    let Mips4ExecutionTransaction::Write {
        physical_address,
        size,
        data,
        byte_enable,
        ..
    } = request
    else {
        unreachable!();
    };
    let _ = state.cache.data_write(
        virtual_address,
        physical_address,
        usize::from(size.bytes()),
        data,
        byte_enable,
        dirty,
    );
}

fn retire_block_frame_control(frame: &mut Mips4BlockFrame) {
    let next_pc = frame.next_pc();
    frame.replace_control(next_pc, next_pc.wrapping_add(4), None);
}

impl<P, F> Mips4BlockRuntime for Mips4ExecutionTarget<P, F>
where
    P: Mips4ExecutionPolicy,
    F: FloatBackend,
{
    fn runtime_abi_v3(&mut self) -> Option<Mips4RuntimeAbiV3> {
        matches!(
            Mips4MmuPrivilegeMode::from_status(self.state.cp0.status()),
            Some(Mips4MmuPrivilegeMode::Kernel)
        )
        .then_some(self.fast_memory_runtime)
        .flatten()
    }

    fn runtime_memory_big_endian(&self) -> bool {
        self.effective_endianness() == Mips4Endianness::Big
    }

    fn execute(
        &mut self,
        frame: &mut Mips4BlockFrame,
        operation: Mips4RuntimeOperation,
    ) -> Mips4RuntimeResult {
        self.block_runtime_action = None;
        if !matches!(operation, Mips4RuntimeOperation::Memory { .. }) {
            self.prepare_runtime_frame(frame, operation);
        }
        let (action, continue_in_block) = match operation {
            Mips4RuntimeOperation::Memory { instruction, raw } => {
                let action = match prepare_memory_with_operands(
                    &self.state,
                    &self.policy,
                    raw,
                    instruction,
                    self.effective_endianness(),
                    frame.read_gpr(raw.rs()),
                    frame.read_gpr(raw.rt()),
                ) {
                    Ok(Mips4MemoryPlan::Read {
                        pending,
                        transaction,
                        virtual_address,
                        cache_policy,
                    }) => match self.complete_block_cached_read(
                        frame,
                        pending,
                        virtual_address,
                        cache_policy,
                        transaction,
                    ) {
                        Ok(()) => return Mips4RuntimeResult::ContinueControl,
                        Err(pending) => {
                            if let Some(runtime) = self.fast_memory_runtime {
                                let Mips4ExecutionTransaction::Read {
                                    physical_address,
                                    size,
                                    kind,
                                    access_type,
                                } = transaction
                                else {
                                    return Mips4RuntimeResult::InternalError;
                                };
                                let request = Mips4FastMemoryReadRequest::new(
                                    physical_address,
                                    size,
                                    kind,
                                    access_type,
                                    frame.retired(),
                                );
                                match runtime.read(request) {
                                    Mips4FastMemoryReadResult::Complete {
                                        value: lanes,
                                        retirement_limit,
                                    } => {
                                        let remaining = retirement_limit
                                            .saturating_sub(request.retired_boundaries());
                                        if remaining == 0 {
                                            return Mips4RuntimeResult::TimelineExhausted;
                                        }
                                        frame.limit_budget(remaining);
                                        let endianness = self.effective_endianness();
                                        let (register, value) = complete_read_value(
                                            &mut self.state,
                                            pending,
                                            lanes,
                                            endianness,
                                        );
                                        frame.write_gpr(register, value);
                                        retire_block_frame_control(frame);
                                        return Mips4RuntimeResult::ContinueControl;
                                    }
                                    Mips4FastMemoryReadResult::TimelineExhausted => {
                                        return Mips4RuntimeResult::TimelineExhausted;
                                    }
                                    Mips4FastMemoryReadResult::InternalError => {
                                        return Mips4RuntimeResult::InternalError;
                                    }
                                    Mips4FastMemoryReadResult::Unavailable => {}
                                }
                            }
                            self.prepare_runtime_frame(frame, operation);
                            self.start_memory_access(
                                Mips4CachedClient::DataRead {
                                    instruction: raw,
                                    pending,
                                },
                                virtual_address,
                                cache_policy,
                                transaction,
                            )
                        }
                    },
                    Ok(Mips4MemoryPlan::Write {
                        pending,
                        transaction,
                        virtual_address,
                        cache_policy,
                    }) => match self.complete_block_cached_write(
                        frame,
                        pending,
                        virtual_address,
                        cache_policy,
                        transaction,
                    ) {
                        Ok(()) => return Mips4RuntimeResult::ContinueControl,
                        Err(pending) => {
                            self.prepare_runtime_frame(frame, operation);
                            self.start_memory_access(
                                Mips4CachedClient::DataWrite {
                                    instruction: raw,
                                    pending,
                                },
                                virtual_address,
                                cache_policy,
                                transaction,
                            )
                        }
                    },
                    Ok(Mips4MemoryPlan::Retire {
                        register_write,
                        clear_llbit,
                    }) => {
                        if let Some((register, value)) = register_write {
                            frame.write_gpr(register, value);
                        }
                        if clear_llbit {
                            self.state.llbit = Mips4LlBit::Clear;
                        }
                        retire_block_frame_control(frame);
                        return Mips4RuntimeResult::ContinueControl;
                    }
                    Err(error) => {
                        self.prepare_runtime_frame(frame, operation);
                        self.memory_error_boundary(error)
                    }
                };
                (action, true)
            }
            Mips4RuntimeOperation::Prefetch { raw } => (self.execute_prefetch(raw), true),
            Mips4RuntimeOperation::Cp0 { raw, operation } => {
                let result = execute_decoded_cp0(&mut self.state, &self.policy, raw, operation);
                (Ok(self.finish_cp0(raw, operation, result)), false)
            }
            Mips4RuntimeOperation::Cp1 { raw, decoded } => {
                let endianness = self.effective_endianness();
                let action = match execute_fpu(
                    &mut self.state,
                    &self.float_backend,
                    &self.policy,
                    raw,
                    decoded,
                    endianness,
                ) {
                    Ok(execution) => Ok(self.finish_fpu(raw, execution)),
                    Err(error) => self.memory_error_boundary(error),
                };
                (
                    action,
                    operation.synchronous_result() == Mips4RuntimeResult::ContinueControl,
                )
            }
            Mips4RuntimeOperation::Cache {
                raw,
                base,
                offset,
                selector,
                operation,
            } => (
                self.execute_cache_operation(raw, base, offset, selector, operation),
                false,
            ),
            Mips4RuntimeOperation::Coprocessor {
                coprocessor,
                requirements,
            } => {
                let present = matches!(coprocessor, Mips4CoprocessorNumber::Cp2)
                    && self.state.config.coprocessors.cp2;
                let access = check_coprocessor_access(
                    self.state.cp0.status(),
                    present,
                    coprocessor,
                    requirements,
                );
                let exception = match access {
                    Mips4InstructionAccess::Execute => Mips4Exception::ReservedInstruction,
                    Mips4InstructionAccess::Exception(exception) => exception,
                    Mips4InstructionAccess::FloatingPointUnimplemented => unreachable!(),
                };
                (Ok(self.exception_boundary(exception, None)), false)
            }
            Mips4RuntimeOperation::Raise(exception) => {
                (Ok(self.exception_boundary(exception, None)), false)
            }
        };

        let action = match action {
            Ok(action) => action,
            Err(_) => return Mips4RuntimeResult::InternalError,
        };
        match action {
            ExecutionTargetAction::Continue => Mips4RuntimeResult::InternalError,
            ExecutionTargetAction::Transaction(transaction) => {
                self.block_runtime_action = Some(ExecutionTargetAction::Transaction(transaction));
                Mips4RuntimeResult::Transaction
            }
            ExecutionTargetAction::Boundary(Mips4ExecutionBoundary::Retired { .. }) => {
                self.refresh_runtime_frame(frame, operation);
                if self.state.standby {
                    Mips4RuntimeResult::Idle
                } else if continue_in_block {
                    Mips4RuntimeResult::ContinueControl
                } else {
                    Mips4RuntimeResult::DispatchControl
                }
            }
            ExecutionTargetAction::Boundary(boundary) => {
                self.refresh_runtime_frame(frame, operation);
                self.block_runtime_action = Some(ExecutionTargetAction::Boundary(boundary));
                Mips4RuntimeResult::Exception
            }
            ExecutionTargetAction::Idle => Mips4RuntimeResult::Idle,
        }
    }

    fn block_guard_valid(&self, guard: &Mips4BlockGuard) -> bool {
        Mips4ExecutionTarget::block_guard_valid(self, guard)
    }

    fn block_guard_epoch(&self) -> u64 {
        self.code_visibility.instruction_generation
    }
}

impl<P, F> ExecutionTarget for Mips4ExecutionTarget<P, F>
where
    P: Mips4ExecutionPolicy,
    F: FloatBackend,
{
    type Transaction = Mips4ExecutionTransaction;
    type Completion = Mips4ExecutionCompletion;
    type Boundary = Mips4ExecutionBoundary;
    type Signal = Mips4ExecutionSignal;
    type Error = Mips4ExecutionTargetError;

    fn reset(&mut self) {
        self.state = Mips4ExecutionState::new(&self.policy)
            .expect("a previously validated cache configuration must remain valid");
        self.pending = None;
        self.pending_error_exception = None;
        self.decode_cache = empty_decode_cache();
        self.code_visibility = Mips4CodeVisibility::default();
        self.fetched_instruction = None;
        self.block_runtime_action = None;
        self.fast_memory_runtime = None;
    }

    fn signal(&mut self, signal: Self::Signal) -> ExecutionTargetSignalAction {
        match signal {
            Mips4ExecutionSignal::ExternalInterrupts(pending) => {
                self.state.external_interrupts = pending;
                self.state.cp0.set_external_interrupts(pending);
                ExecutionTargetSignalAction::Continue
            }
            Mips4ExecutionSignal::InvalidateReservation => {
                self.state.llbit = Mips4LlBit::Clear;
                ExecutionTargetSignalAction::Continue
            }
            Mips4ExecutionSignal::SoftReset => {
                self.latch_error_exception(PendingErrorException {
                    reason: Mips4ErrorException::SoftReset,
                    cache_error: None,
                });
                self.pending = None;
                self.fetched_instruction = None;
                self.state.standby = false;
                ExecutionTargetSignalAction::CancelPending
            }
            Mips4ExecutionSignal::NonMaskableInterrupt => {
                self.latch_error_exception(PendingErrorException {
                    reason: Mips4ErrorException::NonMaskableInterrupt,
                    cache_error: None,
                });
                self.state.standby = false;
                ExecutionTargetSignalAction::Continue
            }
            Mips4ExecutionSignal::CacheError(cache_error) => {
                if self.state.cp0.status().cache_error_disabled() {
                    return ExecutionTargetSignalAction::Continue;
                }
                self.latch_error_exception(PendingErrorException {
                    reason: Mips4ErrorException::CacheError,
                    cache_error: Some(cache_error),
                });
                self.pending = None;
                self.fetched_instruction = None;
                self.state.standby = false;
                ExecutionTargetSignalAction::CancelPending
            }
        }
    }

    fn begin(
        &mut self,
    ) -> Result<ExecutionTargetAction<Self::Transaction, Self::Boundary>, Self::Error> {
        if self.pending.is_some() {
            return Err(Mips4ExecutionTargetError::MissingPendingOperation);
        }
        if let Some(fetched) = self.fetched_instruction.take()
            && fetched.virtual_address == self.state.pc
        {
            return self.execute_instruction(fetched.physical_address, fetched.instruction);
        }
        if let Some(pending) = self.pending_error_exception.take() {
            return Ok(self.error_exception_boundary(pending));
        }
        if self.state.standby {
            if self.state.cp0.cause().interrupt_pending() == 0 {
                return Ok(ExecutionTargetAction::Idle);
            }
            self.state.standby = false;
        }
        let status = self.state.cp0.status();
        let pending = self.state.cp0.cause().interrupt_pending() & status.interrupt_mask();
        if status.interrupts_enabled() && pending != 0 {
            return Ok(self.exception_boundary(Mips4Exception::Interrupt, None));
        }
        self.begin_fetch()
    }

    fn complete(
        &mut self,
        completion: Self::Completion,
    ) -> Result<ExecutionTargetAction<Self::Transaction, Self::Boundary>, Self::Error> {
        match self.pending.take() {
            Some(PendingOperation::InstructionFetch { physical_address }) => {
                self.complete_fetch(physical_address, completion)
            }
            Some(PendingOperation::DataRead {
                instruction,
                pending,
            }) => self.complete_data_read(instruction, pending, completion),
            Some(PendingOperation::DataWrite {
                instruction,
                pending,
            }) => self.complete_data_write(instruction, pending, completion),
            Some(PendingOperation::FpuDataRead {
                instruction,
                pending,
            }) => self.complete_fpu_data_read(instruction, pending, completion),
            Some(PendingOperation::FpuDataWrite { instruction }) => {
                self.complete_fpu_data_write(instruction, completion)
            }
            Some(PendingOperation::CacheRetire { instruction }) => match completion {
                Mips4ExecutionCompletion::ReadData(_) | Mips4ExecutionCompletion::WriteComplete => {
                    Ok(self.retire_sequential(instruction))
                }
                Mips4ExecutionCompletion::BusError => {
                    Ok(self.exception_boundary(Mips4Exception::DataBusError, None))
                }
            },
            Some(PendingOperation::Cached(pending)) => {
                self.complete_cached_access(pending, completion)
            }
            Some(PendingOperation::CacheWriteback(pending)) => {
                self.complete_cache_writeback(pending, completion)
            }
            None => Err(Mips4ExecutionTargetError::MissingPendingOperation),
        }
    }
}

#[cfg(test)]
mod block_tests {
    use se_float::backend::softfloat3::SoftFloat3Backend;

    use crate::cpu::mips4::config::Mips4CacheConfig;
    use crate::cpu::mips4::model::r5000::boot_mode::R5000BootMode;
    use crate::cpu::mips4::model::r5000::execution_policy::R5000ExecutionPolicy;
    use crate::cpu::mips4::model::r5000::profile::R5000Profile;
    use crate::cpu::mips4::model::r5000::revision::R5000Revision;

    use super::*;

    fn target() -> Mips4ExecutionTarget<R5000ExecutionPolicy, SoftFloat3Backend> {
        let profile = R5000Profile::new(
            Mips4Endianness::Big,
            R5000Revision::from_bits(0x21),
            180_000_000,
            Mips4CacheConfig::present(32 * 1024, 32),
            Mips4CacheConfig::present(32 * 1024, 32),
            Mips4CacheConfig::disabled(),
        );
        Mips4ExecutionTarget::new(
            R5000ExecutionPolicy::new(profile, R5000BootMode::from_low_bits(0).unwrap()),
            SoftFloat3Backend::new(),
        )
        .unwrap()
    }

    #[test]
    fn block_guards_track_only_instruction_visibility_mutations() {
        let mut target = target();
        let virtual_address = 0xffff_ffff_9fc0_0000;
        let physical_address = 0x1fc0_0000;
        let config = target.state.cp0.config().bits();
        let _ = target
            .state
            .cp0
            .write(Mips4Cp0Register::Config, u64::from((config & !0x07) | 0x03));
        target.state.pc = virtual_address;
        target.state.next_pc = virtual_address + 4;
        let mut instruction_data = [0; MIPS4_FUNCTIONAL_CACHE_LINE_BYTES];
        instruction_data[..4].copy_from_slice(&0x2402_0001_u32.to_be_bytes());
        let (set, way, _) = target
            .state
            .cache
            .choose_instruction_victim(virtual_address)
            .unwrap();
        target.install_instruction_line(
            set,
            way,
            Mips4CacheLine::from_data(physical_address, virtual_address, instruction_data),
        );
        let block = target.build_block().unwrap().unwrap();
        assert!(target.block_guard_valid(block.guard()));

        let (data_set, data_way, _) = target
            .state
            .cache
            .choose_data_victim(virtual_address)
            .unwrap();
        target.state.cache.install_data(
            data_set,
            data_way,
            Mips4CacheLine::from_data(physical_address, virtual_address, [0; 32]),
        );
        assert!(target.state.cache.data_write(
            virtual_address,
            physical_address,
            4,
            0xffff_ffff,
            0x0f,
            true,
        ));
        assert!(target.block_guard_valid(block.guard()));

        instruction_data[..4].copy_from_slice(&0x2402_0002_u32.to_be_bytes());
        target.install_instruction_line(
            set,
            way,
            Mips4CacheLine::from_data(physical_address, virtual_address, instruction_data),
        );
        assert!(!target.block_guard_valid(block.guard()));
    }

    #[test]
    fn tlb_writes_and_reset_change_only_derived_block_generations() {
        let mut target = target();
        assert_eq!(target.block_key().translation_generation, 0);
        let tlbwi = Mips4Instruction::from_bits((0x10_u32 << 26) | (0x10 << 21) | 0x02);
        let _ = target.finish_cp0(
            tlbwi,
            Mips4Cp0RuntimeOperation::TlbWriteIndexed,
            Mips4Cp0Execution::Retire,
        );
        assert_eq!(target.block_key().translation_generation, 1);
        target.reset();
        assert_eq!(target.block_key().translation_generation, 0);
        assert!(target.code_visibility.instruction_lines.is_empty());
    }

    #[test]
    fn synchronous_cp1_operations_continue_inside_a_block() {
        let mut target = target();
        let status = target.state.cp0.status().bits() | (1 << 29);
        let _ = target
            .state
            .cp0
            .write(Mips4Cp0Register::Status, u64::from(status));
        let raw = Mips4Instruction::from_bits((0x11_u32 << 26) | (0x04 << 21) | (1 << 16));
        let decoded = decode_cp1_instruction(raw).unwrap();
        let mut frame = target.block_frame(2);
        let pc = frame.pc();
        frame.write_gpr(1, 0x1234_5678);

        let result = Mips4BlockRuntime::execute(
            &mut target,
            &mut frame,
            Mips4RuntimeOperation::Cp1 { raw, decoded },
        );

        assert_eq!(result, Mips4RuntimeResult::ContinueControl);
        assert_eq!(frame.pc(), pc.wrapping_add(4));
        assert_eq!(frame.next_pc(), pc.wrapping_add(8));
    }

    #[test]
    fn every_sampled_raw_word_builds_a_verified_dynamic_block() {
        let mut target = target();
        let pc = target.state.pc;
        let physical_address = 0x1fc0_0000;
        let mut verify = |bits: u32| {
            target.fetched_instruction = Some(Mips4FetchedInstruction {
                virtual_address: pc,
                physical_address,
                instruction: Mips4Instruction::from_bits(bits),
            });
            let block = target.take_dynamic_block().unwrap().unwrap();
            assert_eq!(block.instruction_count(), 1, "raw word {bits:#010x}");
            block.verify().unwrap();
        };

        for opcode in 0_u32..64 {
            for rs in 0_u32..32 {
                for function in 0_u32..64 {
                    let bits = (opcode << 26)
                        | (rs << 21)
                        | (((opcode ^ rs ^ function) & 31) << 16)
                        | (((rs.wrapping_mul(7) ^ function) & 31) << 11)
                        | (((opcode.wrapping_mul(3) ^ function) & 31) << 6)
                        | function;
                    verify(bits);
                }
            }
        }

        let mut random = 0x6d2b_79f5_u32;
        for _ in 0..262_144 {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            verify(random);
        }
    }
}
