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
    MIPS4_FUNCTIONAL_CACHE_LINE_BYTES, Mips4CacheAccessPolicy, Mips4CacheLine, line_base,
};
use crate::cpu::mips4::config::Mips4Endianness;
use crate::cpu::mips4::cp0::{Mips4Cp0CacheErr, Mips4Cp0Register};
use crate::cpu::mips4::exception::{
    Mips4CoprocessorNumber, Mips4ErrorException, Mips4ErrorExceptionImage, Mips4Exception,
    Mips4ExceptionImage, Mips4ExceptionRestart,
};
use crate::cpu::mips4::gpr::Mips4GprIndex;
use crate::cpu::mips4::instruction::Mips4Instruction;
use crate::cpu::mips4::instruction::decode::{
    Mips4InstructionClass, Mips4InstructionDecode, decode_instruction,
};
use crate::cpu::mips4::memory::ll_sc::Mips4LlBit;
use crate::cpu::mips4::memory::operation::{
    Mips4InstructionFetch, Mips4MemoryAccessError, Mips4Prefetch, Mips4PrefetchHint,
    Mips4PrefetchResult,
};
use crate::cpu::mips4::mmu::Mips4MmuPrivilegeMode;
use crate::cpu::mips4::tlb::{Mips4TlbAddressMode, Mips4TlbAsid};

use super::bus::{
    Mips4ExecutionAccessKind, Mips4ExecutionCompletion, Mips4ExecutionTransaction,
    Mips4ExecutionTransferSize,
};
use super::cp0::{Mips4Cp0Execution, check_cp0_access, execute_cp0};
use super::fpu::{Mips4FpuExecution, Mips4PendingFpuRead, complete_fpu_read, execute_fpu};
use super::integer::{Mips4CpuExecution, execute_cpu};
use super::memory::{
    Mips4MemoryPlan, Mips4PendingRead, Mips4PendingWrite, complete_read, complete_write,
    prepare_cache_address, prepare_memory,
};
use super::policy::{Mips4ExecutionPolicy, Mips4PrefetchPolicy};
use super::state::Mips4ExecutionConfigError;
use super::state::Mips4ExecutionState;

/// Asynchronous input accepted by functional MIPS IV execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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

enum PendingOperation {
    InstructionFetch,
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

#[derive(Clone, Copy)]
struct PendingErrorException {
    reason: Mips4ErrorException,
    cache_error: Option<Mips4Cp0CacheErr>,
}

enum Mips4CachedClient {
    InstructionFetch,
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

struct Mips4PendingCacheWriteback {
    instruction: Mips4Instruction,
    line: Mips4CacheLine,
    doubleword: u8,
    target: Mips4CacheWritebackTarget,
}

/// Functional MIPS IV execution target parameterized by processor policy and FPU backend.
pub struct Mips4ExecutionTarget<P, F> {
    policy: P,
    float_backend: F,
    state: Mips4ExecutionState,
    pending: Option<PendingOperation>,
    pending_error_exception: Option<PendingErrorException>,
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
                    Mips4CachedClient::InstructionFetch,
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
            Mips4CachedClient::InstructionFetch | Mips4CachedClient::CacheRetire { .. }
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
            Mips4CachedClient::InstructionFetch | Mips4CachedClient::CacheRetire { .. }
        ) {
            self.state
                .cache
                .install_instruction(pending.victim_set, pending.victim_way, line);
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
            Mips4CachedClient::InstructionFetch => self.complete_fetch(completion),
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
        completion: Mips4ExecutionCompletion,
    ) -> Result<
        ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary>,
        Mips4ExecutionTargetError,
    > {
        match completion {
            Mips4ExecutionCompletion::ReadData(data) => {
                let instruction = Mips4Instruction::from_bits(self.instruction_word(data));
                self.execute_instruction(instruction)
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
        instruction: Mips4Instruction,
    ) -> Result<
        ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary>,
        Mips4ExecutionTargetError,
    > {
        match decode_instruction(instruction) {
            Mips4InstructionDecode::Instruction(Mips4InstructionClass::Cpu(decoded)) => {
                if matches!(
                    decoded,
                    crate::cpu::mips4::instruction::decode::Mips4CpuInstruction::Pref
                ) {
                    return self.execute_prefetch(instruction);
                }
                match execute_cpu(&mut self.state, instruction, decoded) {
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
                let result = execute_cp0(&mut self.state, &self.policy, instruction);
                Ok(self.finish_cp0(instruction, result))
            }
            Mips4InstructionDecode::Instruction(Mips4InstructionClass::Coprocessor(
                coprocessor,
            )) => {
                let exception = if self.state.cp0.status().coprocessor_usable(coprocessor) {
                    Mips4Exception::ReservedInstruction
                } else {
                    Mips4Exception::CoprocessorUnusable { coprocessor }
                };
                Ok(self.exception_boundary(exception, None))
            }
            Mips4InstructionDecode::Instruction(Mips4InstructionClass::Fpu(_)) => {
                let endianness = self.effective_endianness();
                let result = execute_fpu(
                    &mut self.state,
                    &self.float_backend,
                    &self.policy,
                    instruction,
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
        if let Err(exception) = check_cp0_access(self.state.cp0.status()) {
            return Ok(self.exception_boundary(exception, None));
        }
        let address = match prepare_cache_address(&self.state, &self.policy, instruction) {
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
        let cache = Mips4CacheInstruction::from_instruction(instruction).unwrap();
        let selector = cache.cache_selector_bits();
        let operation = cache.operation_bits();
        let virtual_address = address.virtual_address;
        let physical_address = address.physical_address;
        match (selector, operation) {
            (0, 0) => {
                if let Some(line) = self
                    .state
                    .cache
                    .primary_index_line_mut(true, virtual_address)
                {
                    line.valid = false;
                    line.dirty = false;
                }
            }
            (0, 1) => self.load_primary_tag(true, virtual_address),
            (0, 2) => self.store_primary_tag(true, virtual_address),
            (0, 4) => {
                if let Some(line) =
                    self.state
                        .cache
                        .primary_hit_line_mut(true, virtual_address, physical_address)
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
                let line =
                    self.state
                        .cache
                        .primary_hit_line_mut(true, virtual_address, physical_address);
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
                    if let Some(line) = self
                        .state
                        .cache
                        .primary_index_line_mut(false, virtual_address)
                    {
                        line.valid = false;
                        line.dirty = false;
                    }
                }
            }
            (1, 1) => self.load_primary_tag(false, virtual_address),
            (1, 2) => self.store_primary_tag(false, virtual_address),
            (1, 3) => {
                if let Some(line) =
                    self.state
                        .cache
                        .primary_hit_line_mut(false, virtual_address, physical_address)
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
                    self.state
                        .cache
                        .primary_hit_line_mut(false, virtual_address, physical_address)
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
                        && let Some(line) = self.state.cache.primary_hit_line_mut(
                            false,
                            virtual_address,
                            physical_address,
                        )
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
        if let Some(line) = self
            .state
            .cache
            .primary_index_line_mut(instruction_cache, virtual_address)
        {
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
        execution: Mips4Cp0Execution,
    ) -> ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary> {
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
            let exception = if matches!(pending.client, Mips4CachedClient::InstructionFetch) {
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
                if let Some(line) = self
                    .state
                    .cache
                    .primary_index_line_mut(instruction_cache, virtual_address)
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
                if let Some(line) = self.state.cache.primary_hit_line_mut(
                    instruction_cache,
                    virtual_address,
                    physical_address,
                ) {
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
        Mips4CachedClient::InstructionFetch => PendingOperation::InstructionFetch,
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
        Mips4CachedClient::InstructionFetch | Mips4CachedClient::CacheRetire { .. }
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
            Some(PendingOperation::InstructionFetch) => self.complete_fetch(completion),
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
