//! Drives complete CPU transitions against a narrow machine-owned timed context.
//!
//! Runtime exits are considered only at safe points. Once a transition timestamp
//! is selected, fetch through normal or exception commit and phase advancement is
//! one atomic region with respect to host-control exits.
//!
//! Safe points give host wake priority over event truncation and the deadline.
//! The initial relaxed interrupt-word load selects control bits only; consuming a
//! host wake performs the acquire operation that publishes host work. Event
//! truncation carries no host payload and remains scheduler-owned. A truncation
//! raised during a transition is observed only after that transition completes.
//! Guest interrupt lines are not interpreted by this execution loop.

use std::error::Error;
use std::fmt;

use se_core::address::PhysAddr;
use se_core::bus::BusFault;
use se_core::interrupt::{EVENT_TRUNCATE, HOST_WAKE};
use se_core::machine::CpuExit;
use se_core::time::VTime;

use crate::commit::CpuCommit;
use crate::cpu::Cpu;
use crate::decode::{DecodeGap, DecodeOutcome, decode};
use crate::exception::ExceptionRequest;
use crate::execute::{ExecuteError, InstructionDisposition, InstructionOutcome, execute};
use crate::memory::{MemoryRequest, TranslationError, translate_compat_kernel_direct};
use crate::pc::PcEffect;
use crate::timing::TimingError;

/// Separates a physical bus result from a failure of the timed machine context.
///
/// The outer error reports synchronization or context failure. The inner
/// [`BusFault`] reports a physical transaction failure; a mapped-device fault does
/// not imply rollback of device state or events produced by that transaction.
pub(crate) type TimedBusResult<T, E> = Result<Result<T, BusFault>, E>;

/// Supplies the machine-owned capabilities needed during one CPU burst.
///
/// An implementation is bound to one CPU's [`se_core::bus::CpuId`] and uses that
/// identity for both timed transaction methods. Each timed method synchronizes
/// the shared scheduler to `time`, releases the scheduler borrow, and only then
/// enters the physical bus. Addresses are physical and 32-bit values follow the
/// guest-big-endian [`se_core::bus::Bus`] contract. No method exposes a raw bus or
/// scheduler handle.
pub(crate) trait CpuRunContext {
    /// Machine-specific invariant or synchronization failure.
    type Error;

    /// Returns the single machine-wide virtual time.
    fn now(&self) -> VTime;

    /// Monotonically synchronizes machine time to an absolute timestamp.
    ///
    /// # Errors
    ///
    /// Returns [`Self::Error`] when the target cannot be reached without violating
    /// machine-time or pending-event invariants.
    fn synchronize_to(&mut self, time: VTime) -> Result<(), Self::Error>;

    /// Synchronizes to `time`, then performs one initiator-bound physical read.
    ///
    /// # Errors
    ///
    /// The outer error reports a context failure. The inner error reports the
    /// [`BusFault`] returned by the physical bus.
    fn read32_at(&mut self, time: VTime, address: PhysAddr) -> TimedBusResult<u32, Self::Error>;

    /// Synchronizes to `time`, then performs one initiator-bound physical write.
    ///
    /// A returned [`BusFault`] does not roll back mapped-device side effects.
    ///
    /// # Errors
    ///
    /// The outer error reports a context failure. The inner error reports the
    /// [`BusFault`] returned by the physical bus.
    fn write32_at(
        &mut self,
        time: VTime,
        address: PhysAddr,
        value: u32,
    ) -> TimedBusResult<(), Self::Error>;
}

/// Identifies which CPU access could not use the supported translation subset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TranslationAccess {
    /// Instruction fetch address.
    InstructionFetch,
    /// `LW` data address.
    LoadWord,
    /// `SW` data address.
    StoreWord,
}

/// Reports a fatal emulator or execution-environment failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CpuRunError<E> {
    /// The clock, phase, machine time, or deadline is inconsistent.
    Timing(TimingError),
    /// The machine-owned timed context rejected an operation.
    Context(E),
    /// A fetched encoding has no typed handler or audited classification.
    Decode(DecodeGap),
    /// Instruction operands require undefined or unpredictable behavior.
    Execute(ExecuteError),
    /// A virtual address lies outside the supported translation subset.
    Translation {
        /// CPU operation that supplied the virtual address.
        access: TranslationAccess,
        /// Address classification failure.
        error: TranslationError,
    },
}

impl<E: fmt::Display> fmt::Display for CpuRunError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timing(error) => write!(formatter, "CPU timing failure: {error}"),
            Self::Context(error) => write!(formatter, "CPU run context failure: {error}"),
            Self::Decode(error) => write!(formatter, "CPU decode gap: {error:?}"),
            Self::Execute(error) => write!(formatter, "CPU execution failure: {error:?}"),
            Self::Translation { access, error } => {
                write!(formatter, "CPU {access:?} translation gap: {error:?}")
            }
        }
    }
}

impl<E: Error + 'static> Error for CpuRunError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Timing(error) => Some(error),
            Self::Context(error) => Some(error),
            Self::Decode(_) | Self::Execute(_) | Self::Translation { .. } => None,
        }
    }
}

impl<E> From<TimingError> for CpuRunError<E> {
    fn from(error: TimingError) -> Self {
        Self::Timing(error)
    }
}

impl Cpu {
    /// Runs complete architectural transitions through the inclusive `deadline`.
    ///
    /// At each safe point, host wake precedes event truncation, then the deadline.
    /// A transition whose boundary equals `deadline` completes before a deadline
    /// exit. Fetch, data bus operations, architectural commit, and phase advance
    /// for one transition share one timestamp and cannot be interrupted by a
    /// runtime exit.
    ///
    /// An error may follow earlier completed transitions or synchronization and
    /// fetch at the failing transition's timestamp. Machine time and bus or device
    /// side effects are not rolled back; the failing transition does not advance
    /// CPU phase or apply a normal CPU commit.
    ///
    /// # Errors
    ///
    /// Returns [`CpuRunError::Timing`] for an unrepresentable phase or inconsistent
    /// machine time and deadline. Returns [`CpuRunError::Context`] when timed
    /// synchronization fails, [`CpuRunError::Decode`] or [`CpuRunError::Execute`]
    /// when guest semantics cannot be determined, and
    /// [`CpuRunError::Translation`] when an address requires unsupported
    /// translation.
    pub(crate) fn run_until<C: CpuRunContext>(
        &mut self,
        context: &mut C,
        deadline: VTime,
    ) -> Result<CpuExit, CpuRunError<C::Error>> {
        loop {
            let now = context.now();
            if deadline < now {
                return Err(TimingError::DeadlineBeforeMachine {
                    deadline,
                    machine_now: now,
                }
                .into());
            }

            let next_boundary = self.next_boundary()?;
            if next_boundary < now {
                return Err(TimingError::PhaseBehindMachine {
                    next_boundary,
                    machine_now: now,
                }
                .into());
            }

            if let Some(exit) = self.host_control_exit() {
                return Ok(exit);
            }

            if next_boundary > deadline {
                if now < deadline {
                    context
                        .synchronize_to(deadline)
                        .map_err(CpuRunError::Context)?;
                }
                return Ok(CpuExit::Deadline);
            }

            // Preflight phase advancement before entering the atomic region so a
            // transition never commits without a representable successor phase.
            let advanced_phase = self.phase().advanced()?;
            self.boundary_for_phase(advanced_phase)?;
            self.execute_at(context, next_boundary)?;
            self.commit_phase(advanced_phase);
        }
    }

    fn host_control_exit(&self) -> Option<CpuExit> {
        let pending = self.interrupt_word().load_relaxed();
        if pending & HOST_WAKE != 0 {
            let consumed = self.interrupt_word().take_host_wake();
            debug_assert!(consumed, "the CPU is the only HostWake consumer");
            if consumed {
                return Some(CpuExit::HostWake);
            }
        }
        if pending & EVENT_TRUNCATE != 0 {
            return Some(CpuExit::Reschedule);
        }
        None
    }

    fn execute_at<C: CpuRunContext>(
        &mut self,
        context: &mut C,
        timestamp: VTime,
    ) -> Result<(), CpuRunError<C::Error>> {
        // This establishes the whole-transition timestamp even when a CPU-side
        // fetch check faults before a physical transaction is issued.
        context
            .synchronize_to(timestamp)
            .map_err(CpuRunError::Context)?;

        let instruction_address = self.pc_state().current();
        if !instruction_address.is_multiple_of(4) {
            self.apply_exception(ExceptionRequest::AddressErrorLoad {
                bad_vaddr: instruction_address,
            });
            return Ok(());
        }

        let physical_address =
            translate_compat_kernel_direct(instruction_address).map_err(|error| {
                CpuRunError::Translation {
                    access: TranslationAccess::InstructionFetch,
                    error,
                }
            })?;
        let raw = match context
            .read32_at(timestamp, physical_address)
            .map_err(CpuRunError::Context)?
        {
            Ok(raw) => raw,
            Err(fault) => {
                self.apply_exception(instruction_bus_exception(fault));
                return Ok(());
            }
        };

        let instruction = match decode(raw) {
            DecodeOutcome::Instruction(instruction) => instruction,
            DecodeOutcome::ReservedEncoding { .. } => {
                self.apply_exception(ExceptionRequest::ReservedInstruction);
                return Ok(());
            }
            DecodeOutcome::ImplementationGap(error) => return Err(CpuRunError::Decode(error)),
        };

        match execute(self, instruction).map_err(CpuRunError::Execute)? {
            InstructionDisposition::Architectural(outcome) => {
                self.apply_instruction_outcome(outcome);
                Ok(())
            }
            InstructionDisposition::Memory(request) => {
                self.complete_memory(context, timestamp, request)
            }
        }
    }

    fn apply_instruction_outcome(&mut self, outcome: InstructionOutcome) {
        match outcome {
            InstructionOutcome::Commit(commit) => self.apply_commit(commit),
            InstructionOutcome::Exception(request) => self.apply_exception(request),
        }
    }

    fn complete_memory<C: CpuRunContext>(
        &mut self,
        context: &mut C,
        timestamp: VTime,
        request: MemoryRequest,
    ) -> Result<(), CpuRunError<C::Error>> {
        match request {
            MemoryRequest::LoadWord {
                destination,
                virtual_address,
            } => {
                let physical_address =
                    translate_compat_kernel_direct(virtual_address).map_err(|error| {
                        CpuRunError::Translation {
                            access: TranslationAccess::LoadWord,
                            error,
                        }
                    })?;
                let value = match context
                    .read32_at(timestamp, physical_address)
                    .map_err(CpuRunError::Context)?
                {
                    Ok(value) => value,
                    Err(fault) => {
                        self.apply_exception(data_bus_exception(fault));
                        return Ok(());
                    }
                };
                let extended = (i64::from(value as i32)) as u64;
                self.apply_commit(
                    CpuCommit::new(PcEffect::Sequential).with_gpr_write(destination, extended),
                );
                Ok(())
            }
            MemoryRequest::StoreWord {
                value,
                virtual_address,
            } => {
                let physical_address =
                    translate_compat_kernel_direct(virtual_address).map_err(|error| {
                        CpuRunError::Translation {
                            access: TranslationAccess::StoreWord,
                            error,
                        }
                    })?;
                match context
                    .write32_at(timestamp, physical_address, value)
                    .map_err(CpuRunError::Context)?
                {
                    Ok(()) => self.apply_commit(CpuCommit::new(PcEffect::Sequential)),
                    Err(fault) => self.apply_exception(data_bus_exception(fault)),
                }
                Ok(())
            }
        }
    }
}

fn instruction_bus_exception(fault: BusFault) -> ExceptionRequest {
    match fault {
        BusFault::Unmapped => ExceptionRequest::InstructionBusError,
        BusFault::Fault => ExceptionRequest::InstructionBusError,
    }
}

fn data_bus_exception(fault: BusFault) -> ExceptionRequest {
    match fault {
        BusFault::Unmapped => ExceptionRequest::DataBusError,
        BusFault::Fault => ExceptionRequest::DataBusError,
    }
}
