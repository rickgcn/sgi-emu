//! Owns CPU architectural state, absolute retirement phase, and control delivery.
//!
//! Execution borrows [`Cpu`] immutably and produces either a [`CpuCommit`] or an
//! [`ExceptionRequest`]. [`Cpu::apply_commit`] is the instruction-commit mutation
//! path, while [`Cpu::apply_exception`] is the distinct exception-entry path.
//! Instruction handlers cannot expose partially applied
//! architectural state. The processor clock and phase place those transitions on
//! machine time. The interrupt word supplies live host-control and guest-line
//! inputs rather than stored guest architectural state.

use crate::commit::{CpuCommit, PcCommitEffect};
use crate::cp0::Cp0;
use crate::exception::{ExceptionLocation, ExceptionRequest};
use crate::gpr::{GprFile, Reg};
use crate::interrupt::external_pending_ip;
use crate::pc::PcState;
use crate::timing::{ProcessorClock, RetirementPhase, TimingError};
use se_core::interrupt::InterruptWord;
use se_core::time::VTime;

/// Holds one CPU's architectural state, retirement phase, and control-delivery word.
///
/// Instruction semantics receive shared access and cannot mutate architectural
/// fields. [`Self::apply_commit`] applies instruction commits, while
/// [`Self::apply_exception`] applies exception entry.
#[derive(Debug)]
#[cfg_attr(test, derive(Clone))]
pub(crate) struct Cpu {
    gpr: GprFile,
    pc: PcState,
    cp0: Cp0,
    clock: ProcessorClock,
    phase: RetirementPhase,
    interrupt_word: InterruptWord,
}

impl Cpu {
    /// Constructs a CPU scheduled to transition on the first PClk edge.
    ///
    /// The execution-control word starts cleared.
    pub(crate) fn from_parts(gpr: GprFile, pc: PcState, cp0: Cp0, clock: ProcessorClock) -> Self {
        Self {
            gpr,
            pc,
            cp0,
            clock,
            phase: RetirementPhase::initial(),
            interrupt_word: InterruptWord::new(),
        }
    }

    pub(crate) fn read_gpr(&self, reg: Reg) -> u64 {
        self.gpr.read(reg)
    }

    pub(crate) const fn pc_state(&self) -> &PcState {
        &self.pc
    }

    pub(crate) const fn cp0(&self) -> &Cp0 {
        &self.cp0
    }

    pub(crate) const fn next_pclk_tick(&self) -> u64 {
        self.phase.next_pclk_tick()
    }

    pub(crate) fn next_boundary(&self) -> Result<VTime, TimingError> {
        self.clock.boundary(self.phase)
    }

    pub(crate) fn boundary_for_phase(&self, phase: RetirementPhase) -> Result<VTime, TimingError> {
        self.clock.boundary(phase)
    }

    pub(crate) const fn phase(&self) -> RetirementPhase {
        self.phase
    }

    pub(crate) fn commit_phase(&mut self, phase: RetirementPhase) {
        self.phase = phase;
    }

    pub(crate) const fn interrupt_word(&self) -> &InterruptWord {
        &self.interrupt_word
    }

    /// Returns the live external portion of the architectural `Cause.IP` view.
    pub(crate) fn cause_pending_ip(&self) -> u8 {
        external_pending_ip(self.interrupt_word.load_relaxed())
    }

    /// Applies a write-set for one instruction's architectural commit.
    ///
    /// This is the only mutation path for instruction commits. Exception-return
    /// CP0 and PC effects are applied without an intervening safe point.
    ///
    /// # Panics
    ///
    /// Panics if the commit contains a delayed transfer while the current
    /// instruction already occupies a delay slot.
    pub(crate) fn apply_commit(&mut self, commit: CpuCommit) {
        let (gpr, cp0, pc) = commit.into_parts();
        match pc {
            PcCommitEffect::Normal(effect) => self.pc.apply(effect),
            PcCommitEffect::ExceptionReturn { target } => self.pc.return_from_exception(target),
        }
        if let Some(effect) = cp0 {
            self.cp0.apply_effect(effect);
        }
        if let Some(write) = gpr {
            let (destination, value) = write.into_parts();
            self.gpr.write(destination, value);
        }
    }

    /// Applies one exception as a complete architectural transition.
    ///
    /// This captures the current and delay-slot location before updating `CP0`, then
    /// redirects the program counter to the selected vector. Redirection is
    /// infallible, so guest input cannot leave partially applied `CP0` and PC state.
    pub(crate) fn apply_exception(&mut self, request: ExceptionRequest) {
        let location = ExceptionLocation::from_pc_state(&self.pc);
        let vector = self.cp0.apply_exception(request, location);
        self.pc.enter_exception(vector);
    }
}

// Execution-control delivery is transient and deliberately excluded from test
// state comparisons. Cloning a test CPU shares the delivery word, so pending
// control bits cannot be treated as copied architectural state.
#[cfg(test)]
impl PartialEq for Cpu {
    fn eq(&self, other: &Self) -> bool {
        self.gpr == other.gpr
            && self.pc == other.pc
            && self.cp0 == other.cp0
            && self.clock == other.clock
            && self.phase == other.phase
    }
}

#[cfg(test)]
impl Eq for Cpu {}

#[cfg(test)]
mod tests {
    use super::Cpu;
    use crate::commit::CpuCommit;
    use crate::cp0::Cp0;
    use crate::decode::Instruction;
    use crate::exception::{ExceptionCode, ExceptionRequest};
    use crate::execute::{InstructionDisposition, InstructionOutcome, execute};
    use crate::gpr::{GprFile, Reg};
    use crate::pc::{PcEffect, PcState};
    use crate::timing::ProcessorClock;

    fn reg(index: u8) -> Reg {
        Reg::new(index).expect("test register index must be architectural")
    }

    fn cpu_at(current: u64, bev: bool) -> Cpu {
        Cpu::from_parts(
            GprFile::new(),
            PcState::new(current),
            Cp0::synthetic_test_state(bev),
            ProcessorClock::new(1_000_000_000).unwrap(),
        )
    }

    #[test]
    fn execute_builds_a_candidate_before_normal_retirement() {
        let source = reg(1);
        let destination = reg(2);
        let mut gpr = GprFile::new();
        gpr.write(source, 0x1234);
        let mut cpu = Cpu::from_parts(
            gpr,
            PcState::new(0x1000),
            Cp0::synthetic_test_state(false),
            ProcessorClock::new(1_000_000_000).unwrap(),
        );
        let instruction = Instruction::Ori {
            rt: destination,
            rs: source,
            immediate: 0x00f0,
        };

        let disposition = execute(&cpu, instruction).expect("ORI must execute normally");
        let InstructionDisposition::Architectural(outcome) = disposition else {
            panic!("ORI must not require timed memory");
        };
        let InstructionOutcome::Commit(commit) = outcome else {
            panic!("ORI must produce a normal commit");
        };

        assert_eq!(cpu.read_gpr(destination), 0);
        assert_eq!(cpu.pc_state().current(), 0x1000);

        cpu.apply_commit(commit);

        assert_eq!(cpu.read_gpr(destination), 0x12f4);
        assert_eq!(cpu.pc_state().current(), 0x1004);
    }

    #[test]
    fn normal_exception_records_location_and_enters_the_general_vector() {
        let mut cpu = cpu_at(0x1000, false);

        cpu.apply_exception(ExceptionRequest::IntegerOverflow);

        assert_eq!(cpu.cp0().epc(), 0x1000);
        assert!(!cpu.cp0().branch_delay());
        assert_eq!(cpu.cp0().exception_code(), ExceptionCode::IntegerOverflow);
        assert!(cpu.cp0().exl());
        assert_eq!(cpu.pc_state().current(), 0xffff_ffff_8000_0180);
        assert_eq!(cpu.pc_state().next(), 0xffff_ffff_8000_0184);
        assert_eq!(cpu.pc_state().delay_slot_of(), None);
    }

    #[test]
    fn bev_selects_the_bootstrap_general_vector() {
        let mut cpu = cpu_at(0x1000, true);

        cpu.apply_exception(ExceptionRequest::Syscall);

        assert!(cpu.cp0().bev());
        assert_eq!(cpu.pc_state().current(), 0xffff_ffff_bfc0_0380);
        assert_eq!(cpu.pc_state().next(), 0xffff_ffff_bfc0_0384);
    }

    #[test]
    fn delay_slot_exception_records_the_branch_origin() {
        let mut cpu = cpu_at(0x1000, false);
        cpu.apply_commit(CpuCommit::new(PcEffect::DelayedTransfer {
            after_delay_slot: 0x2000,
        }));

        cpu.apply_exception(ExceptionRequest::Breakpoint);

        assert_eq!(cpu.cp0().epc(), 0x1000);
        assert!(cpu.cp0().branch_delay());
        assert!(cpu.cp0().exl());
        assert_eq!(cpu.pc_state().current(), 0xffff_ffff_8000_0180);
        assert_eq!(cpu.pc_state().delay_slot_of(), None);
    }

    #[test]
    fn nested_exception_preserves_epc_and_branch_delay() {
        let mut cpu = cpu_at(0x1000, false);
        cpu.apply_commit(CpuCommit::new(PcEffect::DelayedTransfer {
            after_delay_slot: 0x2000,
        }));
        cpu.apply_exception(ExceptionRequest::Syscall);

        cpu.apply_exception(ExceptionRequest::Breakpoint);

        assert_eq!(cpu.cp0().epc(), 0x1000);
        assert!(cpu.cp0().branch_delay());
        assert_eq!(cpu.cp0().exception_code(), ExceptionCode::Breakpoint);
        assert!(cpu.cp0().exl());
        assert_eq!(cpu.pc_state().current(), 0xffff_ffff_8000_0180);
    }

    #[test]
    fn interrupt_and_coprocessor_unusable_share_precise_exception_entry() {
        let mut interrupt = cpu_at(0x1000, false);
        interrupt.apply_commit(CpuCommit::new(PcEffect::DelayedTransfer {
            after_delay_slot: 0x2000,
        }));

        interrupt.apply_exception(ExceptionRequest::Interrupt);

        assert_eq!(interrupt.cp0().exception_code(), ExceptionCode::Interrupt);
        assert_eq!(interrupt.cp0().epc(), 0x1000);
        assert!(interrupt.cp0().branch_delay());
        assert_eq!(interrupt.pc_state().current(), 0xffff_ffff_8000_0180);

        let mut unusable = cpu_at(0x3000, false);
        unusable.apply_exception(ExceptionRequest::CoprocessorUnusable { coprocessor: 3 });

        assert_eq!(
            unusable.cp0().exception_code(),
            ExceptionCode::CoprocessorUnusable
        );
        assert_eq!(unusable.cp0().coprocessor_error(), 3);
        assert_eq!(unusable.cp0().epc(), 0x3000);
        assert!(!unusable.cp0().branch_delay());
        assert_eq!(unusable.pc_state().current(), 0xffff_ffff_8000_0180);
    }

    #[test]
    fn address_exception_updates_bad_vaddr() {
        let mut cpu = cpu_at(0x1000, false);

        cpu.apply_exception(ExceptionRequest::AddressErrorLoad { bad_vaddr: 0x123 });

        assert_eq!(cpu.cp0().bad_vaddr(), 0x123);
        assert_eq!(cpu.cp0().exception_code(), ExceptionCode::AddressErrorLoad);
    }

    #[test]
    fn current_policy_preserves_bad_vaddr_for_non_address_exceptions() {
        let requests = [
            ExceptionRequest::Interrupt,
            ExceptionRequest::IntegerOverflow,
            ExceptionRequest::Syscall,
            ExceptionRequest::Breakpoint,
            ExceptionRequest::ReservedInstruction,
            ExceptionRequest::InstructionBusError,
            ExceptionRequest::DataBusError,
            ExceptionRequest::CoprocessorUnusable { coprocessor: 0 },
        ];

        for request in requests {
            let mut cpu = cpu_at(0x1000, false);
            cpu.apply_exception(ExceptionRequest::AddressErrorStore {
                bad_vaddr: 0xdead_beef,
            });

            // The architecture leaves BadVAddr undefined for these exceptions;
            // preserving the previous value is the deterministic local policy.
            cpu.apply_exception(request);

            assert_eq!(cpu.cp0().bad_vaddr(), 0xdead_beef);
            assert_eq!(cpu.cp0().exception_code(), request.exception_code());
        }
    }
}
