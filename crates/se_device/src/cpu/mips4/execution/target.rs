//! Functional MIPS IV execution target.

use core::fmt;

use se_float::backend::FloatBackend;

use crate::cpu::execution::target::{ExecutionBoundary, ExecutionTarget, ExecutionTargetAction};
use crate::cpu::mips4::branch::Mips4BranchDecision;
use crate::cpu::mips4::config::Mips4Endianness;
use crate::cpu::mips4::exception::{
    Mips4CoprocessorNumber, Mips4Exception, Mips4ExceptionImage, Mips4ExceptionRestart,
};
use crate::cpu::mips4::gpr::Mips4GprIndex;
use crate::cpu::mips4::instruction::Mips4Instruction;
use crate::cpu::mips4::instruction::decode::{
    Mips4InstructionClass, Mips4InstructionDecode, decode_instruction,
};
use crate::cpu::mips4::memory::ll_sc::Mips4LlBit;
use crate::cpu::mips4::memory::operation::{Mips4InstructionFetch, Mips4MemoryAccessError};
use crate::cpu::mips4::mmu::Mips4MmuPrivilegeMode;
use crate::cpu::mips4::tlb::{Mips4TlbAddressMode, Mips4TlbAsid};

use super::bus::{
    Mips4ExecutionAccessKind, Mips4ExecutionCompletion, Mips4ExecutionTransaction,
    Mips4ExecutionTransferSize,
};
use super::cp0::{Mips4Cp0Execution, execute_cache, execute_cp0};
use super::fpu::{Mips4FpuExecution, Mips4PendingFpuRead, complete_fpu_read, execute_fpu};
use super::integer::{Mips4CpuExecution, execute_cpu};
use super::memory::{
    Mips4MemoryPlan, Mips4PendingRead, Mips4PendingWrite, complete_read, complete_write,
    prepare_memory,
};
use super::policy::Mips4ExecutionPolicy;
use super::state::Mips4ExecutionState;

/// Asynchronous input accepted by functional MIPS IV execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4ExecutionSignal {
    /// Replace external Cause IP line levels.
    ExternalInterrupts(u8),

    /// Clear an outstanding LL/SC reservation.
    InvalidateReservation,
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
}

/// Functional MIPS IV execution target parameterized by processor policy and FPU backend.
pub struct Mips4ExecutionTarget<P, F> {
    policy: P,
    float_backend: F,
    state: Mips4ExecutionState,
    pending: Option<PendingOperation>,
}

impl<P, F> Mips4ExecutionTarget<P, F>
where
    P: Mips4ExecutionPolicy,
    F: FloatBackend,
{
    /// Creates a functional target in reset state.
    pub fn new(policy: P, float_backend: F) -> Self {
        let state = Mips4ExecutionState::new(&policy);
        Self {
            policy,
            float_backend,
            state,
            pending: None,
        }
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
                self.pending = Some(PendingOperation::InstructionFetch);
                Ok(ExecutionTargetAction::Transaction(
                    Mips4ExecutionTransaction::Read {
                        physical_address: fetch.physical_address(),
                        size: Mips4ExecutionTransferSize::Word,
                        kind: Mips4ExecutionAccessKind::InstructionFetch,
                        access_type,
                    },
                ))
            }
            Err(error) => self.memory_error_boundary(error),
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
                            }) => {
                                self.pending = Some(PendingOperation::DataRead {
                                    instruction,
                                    pending,
                                });
                                Ok(ExecutionTargetAction::Transaction(transaction))
                            }
                            Ok(Mips4MemoryPlan::Write {
                                pending,
                                transaction,
                            }) => {
                                self.pending = Some(PendingOperation::DataWrite {
                                    instruction,
                                    pending,
                                });
                                Ok(ExecutionTargetAction::Transaction(transaction))
                            }
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
                let result = execute_cache(&mut self.state, instruction);
                Ok(self.finish_cp0(instruction, result))
            }
            Mips4InstructionDecode::UndefinedResult => Ok(self.retire_sequential(instruction)),
        }
    }

    fn finish_cp0(
        &mut self,
        instruction: Mips4Instruction,
        execution: Mips4Cp0Execution,
    ) -> ExecutionTargetAction<Mips4ExecutionTransaction, Mips4ExecutionBoundary> {
        match execution {
            Mips4Cp0Execution::Retire => self.retire_sequential(instruction),
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
            } => {
                self.pending = Some(PendingOperation::FpuDataRead {
                    instruction,
                    pending,
                });
                ExecutionTargetAction::Transaction(transaction)
            }
            Mips4FpuExecution::Write { transaction } => {
                self.pending = Some(PendingOperation::FpuDataWrite { instruction });
                ExecutionTargetAction::Transaction(transaction)
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
        self.policy
            .endianness()
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
                    | Mips4FpuExecution::Write { .. } => {
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
        self.state = Mips4ExecutionState::new(&self.policy);
        self.pending = None;
    }

    fn signal(&mut self, signal: Self::Signal) {
        match signal {
            Mips4ExecutionSignal::ExternalInterrupts(pending) => {
                self.state.external_interrupts = pending;
                self.state.cp0.set_external_interrupts(pending);
            }
            Mips4ExecutionSignal::InvalidateReservation => {
                self.state.llbit = Mips4LlBit::Clear;
            }
        }
    }

    fn begin(
        &mut self,
    ) -> Result<ExecutionTargetAction<Self::Transaction, Self::Boundary>, Self::Error> {
        if self.pending.is_some() {
            return Err(Mips4ExecutionTargetError::MissingPendingOperation);
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
            None => Err(Mips4ExecutionTargetError::MissingPendingOperation),
        }
    }
}
