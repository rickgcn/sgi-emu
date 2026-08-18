//! Applies the CP0 state transitions required for exception entry and return.
//!
//! [`Cp0`] stores only fields with immediate semantic consumers. Exception
//! requests supply the cause and optional fault address, while exception
//! locations supply the precise `EPC` and `BD` context. External hardware
//! interrupt pending bits remain live inputs rather than stored Cause state. The
//! module exposes neither packed CP0 registers nor architectural reset values.

use crate::exception::{ExceptionCode, ExceptionLocation, ExceptionRequest};

const NORMAL_GENERAL_EXCEPTION_VECTOR: u64 = 0xffff_ffff_8000_0180;
const BOOTSTRAP_GENERAL_EXCEPTION_VECTOR: u64 = 0xffff_ffff_bfc0_0380;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Status {
    interrupt_enable: bool,
    exl: bool,
    erl: bool,
    interrupt_mask: u8,
    operating_mode: OperatingMode,
    cu0: bool,
    bev: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Cause {
    exception_code: ExceptionCode,
    branch_delay: bool,
    coprocessor_error: u8,
}

/// Identifies the mode selected by `Status.KSU` when no exception level is active.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OperatingMode {
    /// Kernel mode.
    Kernel,
    /// Supervisor mode.
    Supervisor,
    /// User mode.
    User,
}

/// Identifies which exception level an exception return clears.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExceptionReturnLevel {
    /// Clears `Status.EXL` and returns through `EPC`.
    Exception,
    /// Clears `Status.ERL` and returns through `ErrorEPC`.
    Error,
}

/// Binds an exception-return target to the exception level that selected it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ExceptionReturnDecision {
    target: u64,
    level: ExceptionReturnLevel,
}

impl ExceptionReturnDecision {
    pub(crate) const fn target(self) -> u64 {
        self.target
    }

    pub(crate) const fn level(self) -> ExceptionReturnLevel {
        self.level
    }
}

/// Describes a CP0 mutation carried by an instruction commit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Cp0Effect {
    /// Clears the exception level already selected by an immutable return decision.
    ExceptionReturn { level: ExceptionReturnLevel },
}

/// Stores the CP0 subset consumed and produced by exception entry and return.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Cp0 {
    status: Status,
    cause: Cause,
    epc: u64,
    error_epc: u64,
    bad_vaddr: u64,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SyntheticCp0State {
    interrupt_enable: bool,
    exl: bool,
    erl: bool,
    interrupt_mask: u8,
    operating_mode: OperatingMode,
    cu0: bool,
    bev: bool,
    epc: u64,
    error_epc: u64,
}

#[cfg(test)]
impl SyntheticCp0State {
    pub(crate) const fn new(bev: bool) -> Self {
        Self {
            interrupt_enable: false,
            exl: false,
            erl: false,
            interrupt_mask: 0,
            operating_mode: OperatingMode::Kernel,
            cu0: false,
            bev,
            epc: 0,
            error_epc: 0,
        }
    }

    pub(crate) const fn with_interrupts(mut self, enabled: bool, mask: u8) -> Self {
        self.interrupt_enable = enabled;
        self.interrupt_mask = mask;
        self
    }

    pub(crate) const fn with_exception_levels(mut self, exl: bool, erl: bool) -> Self {
        self.exl = exl;
        self.erl = erl;
        self
    }

    pub(crate) const fn with_operating_mode(mut self, mode: OperatingMode, cu0: bool) -> Self {
        self.operating_mode = mode;
        self.cu0 = cu0;
        self
    }

    pub(crate) const fn with_return_addresses(mut self, epc: u64, error_epc: u64) -> Self {
        self.epc = epc;
        self.error_epc = error_epc;
        self
    }
}

impl Cp0 {
    // Constructs explicit semantic-test pre-state. The selected `Cause`, `EPC`,
    // and `BadVAddr` values are test inputs, not R10000 reset semantics.
    #[cfg(test)]
    pub(crate) const fn synthetic_test_state(bev: bool) -> Self {
        Self::synthetic_test_state_with(SyntheticCp0State::new(bev))
    }

    #[cfg(test)]
    pub(crate) const fn synthetic_test_state_with(state: SyntheticCp0State) -> Self {
        Self {
            status: Status {
                interrupt_enable: state.interrupt_enable,
                exl: state.exl,
                erl: state.erl,
                interrupt_mask: state.interrupt_mask,
                operating_mode: state.operating_mode,
                cu0: state.cu0,
                bev: state.bev,
            },
            cause: Cause {
                exception_code: ExceptionCode::ReservedInstruction,
                branch_delay: false,
                coprocessor_error: 0,
            },
            epc: state.epc,
            error_epc: state.error_epc,
            bad_vaddr: 0,
        }
    }

    pub(crate) const fn interrupt_enable(&self) -> bool {
        self.status.interrupt_enable
    }

    pub(crate) const fn exl(&self) -> bool {
        self.status.exl
    }

    pub(crate) const fn erl(&self) -> bool {
        self.status.erl
    }

    pub(crate) const fn interrupt_mask(&self) -> u8 {
        self.status.interrupt_mask
    }

    pub(crate) const fn effective_mode(&self) -> OperatingMode {
        if self.status.exl || self.status.erl {
            OperatingMode::Kernel
        } else {
            self.status.operating_mode
        }
    }

    pub(crate) const fn cp0_usable(&self) -> bool {
        matches!(self.effective_mode(), OperatingMode::Kernel) || self.status.cu0
    }

    pub(crate) const fn bev(&self) -> bool {
        self.status.bev
    }

    pub(crate) const fn exception_code(&self) -> ExceptionCode {
        self.cause.exception_code
    }

    pub(crate) const fn branch_delay(&self) -> bool {
        self.cause.branch_delay
    }

    pub(crate) const fn coprocessor_error(&self) -> u8 {
        self.cause.coprocessor_error
    }

    pub(crate) const fn epc(&self) -> u64 {
        self.epc
    }

    pub(crate) const fn error_epc(&self) -> u64 {
        self.error_epc
    }

    pub(crate) const fn bad_vaddr(&self) -> u64 {
        self.bad_vaddr
    }

    /// Returns the exception-return target and level selected from immutable state.
    ///
    /// `Status.ERL` selects `ErrorEPC` and the error level. Otherwise, `EPC` and
    /// the exception level are selected regardless of `Status.EXL`.
    pub(crate) const fn exception_return_decision(&self) -> ExceptionReturnDecision {
        if self.status.erl {
            ExceptionReturnDecision {
                target: self.error_epc,
                level: ExceptionReturnLevel::Error,
            }
        } else {
            ExceptionReturnDecision {
                target: self.epc,
                level: ExceptionReturnLevel::Exception,
            }
        }
    }

    /// Reports whether a current pending-IP view can request an interrupt.
    ///
    /// Acceptance requires `Status.IE`, clear `Status.EXL` and `Status.ERL`, and
    /// at least one bit shared by `Status.IM` and `pending_ip`. `Status.KSU` and
    /// `Status.CU0` do not participate.
    pub(crate) const fn interrupt_eligible(&self, pending_ip: u8) -> bool {
        self.status.interrupt_enable
            && !self.status.exl
            && !self.status.erl
            && (self.status.interrupt_mask & pending_ip) != 0
    }

    /// Applies one exception and returns the selected general exception vector.
    ///
    /// When `Status.EXL` is clear, this captures `EPC` and `Cause.BD` from
    /// `location`; when it is set, both fields remain protected. Every request
    /// updates `Cause.ExcCode` and sets `Status.EXL`. A Coprocessor Unusable
    /// request updates `Cause.CE`; other requests preserve its previous value as a
    /// deterministic policy because the architecture leaves it undefined. A
    /// request carrying a fault address writes `BadVAddr`. For other requests this
    /// implementation leaves the stored value untouched, but the architecture does
    /// not define that value.
    ///
    /// `Status.BEV = 0` selects `0xffff_ffff_8000_0180`; `Status.BEV = 1`
    /// selects `0xffff_ffff_bfc0_0380`.
    pub(crate) fn apply_exception(
        &mut self,
        request: ExceptionRequest,
        location: ExceptionLocation,
    ) -> u64 {
        let vector = self.general_exception_vector();

        self.cause.exception_code = request.exception_code();
        if let Some(coprocessor) = request.coprocessor() {
            self.cause.coprocessor_error = coprocessor;
        }
        if !self.status.exl {
            let (epc, branch_delay) = location.exception_program_counter();
            self.epc = epc;
            self.cause.branch_delay = branch_delay;
        }
        // The R10000 architecture leaves BadVAddr undefined for non-address
        // exceptions. Preserve its previous value as the deterministic policy.
        if let Some(bad_vaddr) = request.bad_vaddr() {
            self.bad_vaddr = bad_vaddr;
        }
        self.status.exl = true;

        vector
    }

    /// Applies a staged CP0 effect without re-evaluating its return decision.
    pub(crate) fn apply_effect(&mut self, effect: Cp0Effect) {
        match effect {
            Cp0Effect::ExceptionReturn {
                level: ExceptionReturnLevel::Exception,
            } => self.status.exl = false,
            Cp0Effect::ExceptionReturn {
                level: ExceptionReturnLevel::Error,
            } => self.status.erl = false,
        }
    }

    const fn general_exception_vector(&self) -> u64 {
        if self.status.bev {
            BOOTSTRAP_GENERAL_EXCEPTION_VECTOR
        } else {
            NORMAL_GENERAL_EXCEPTION_VECTOR
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Cp0, ExceptionReturnLevel, OperatingMode, SyntheticCp0State};
    use crate::exception::{ExceptionCode, ExceptionLocation, ExceptionRequest};
    use crate::pc::PcState;

    fn cp0(state: SyntheticCp0State) -> Cp0 {
        Cp0::synthetic_test_state_with(state)
    }

    #[test]
    fn effective_mode_forces_kernel_while_an_exception_level_is_active() {
        let supervisor = cp0(
            SyntheticCp0State::new(false).with_operating_mode(OperatingMode::Supervisor, false)
        );
        let user_exception = cp0(SyntheticCp0State::new(false)
            .with_operating_mode(OperatingMode::User, false)
            .with_exception_levels(true, false));
        let user_error = cp0(SyntheticCp0State::new(false)
            .with_operating_mode(OperatingMode::User, false)
            .with_exception_levels(false, true));

        assert_eq!(supervisor.effective_mode(), OperatingMode::Supervisor);
        assert_eq!(user_exception.effective_mode(), OperatingMode::Kernel);
        assert_eq!(user_error.effective_mode(), OperatingMode::Kernel);
        assert!(!supervisor.cp0_usable());
        assert!(user_exception.cp0_usable());
        assert!(user_error.cp0_usable());
    }

    #[test]
    fn cp0_usability_in_user_mode_follows_cu0() {
        let disabled =
            cp0(SyntheticCp0State::new(false).with_operating_mode(OperatingMode::User, false));
        let enabled =
            cp0(SyntheticCp0State::new(false).with_operating_mode(OperatingMode::User, true));

        assert!(!disabled.cp0_usable());
        assert!(enabled.cp0_usable());
    }

    #[test]
    fn interrupt_eligibility_requires_ie_clear_levels_and_an_unmasked_pending_bit() {
        let enabled = cp0(SyntheticCp0State::new(false).with_interrupts(true, 1 << 2));
        let disabled = cp0(SyntheticCp0State::new(false).with_interrupts(false, 1 << 2));
        let masked = cp0(SyntheticCp0State::new(false).with_interrupts(true, 0));
        let exception = cp0(SyntheticCp0State::new(false)
            .with_interrupts(true, 1 << 2)
            .with_exception_levels(true, false));
        let error = cp0(SyntheticCp0State::new(false)
            .with_interrupts(true, 1 << 2)
            .with_exception_levels(false, true));
        let user = cp0(SyntheticCp0State::new(false)
            .with_interrupts(true, 1 << 2)
            .with_operating_mode(OperatingMode::User, false));

        assert!(enabled.interrupt_enable());
        assert_eq!(enabled.interrupt_mask(), 1 << 2);
        assert!(enabled.interrupt_eligible(1 << 2));
        assert!(!disabled.interrupt_eligible(1 << 2));
        assert!(!masked.interrupt_eligible(1 << 2));
        assert!(!enabled.interrupt_eligible(1 << 3));
        assert!(!exception.interrupt_eligible(1 << 2));
        assert!(!error.interrupt_eligible(1 << 2));
        assert!(user.interrupt_eligible(1 << 2));
    }

    #[test]
    fn return_decision_gives_erl_priority_over_exl() {
        let cp0 = cp0(SyntheticCp0State::new(false)
            .with_exception_levels(true, true)
            .with_return_addresses(0x1000, 0x2000));

        let decision = cp0.exception_return_decision();

        assert_eq!(decision.target(), 0x2000);
        assert_eq!(decision.level(), ExceptionReturnLevel::Error);
        assert_eq!(cp0.epc(), 0x1000);
        assert_eq!(cp0.error_epc(), 0x2000);
    }

    #[test]
    fn coprocessor_unusable_updates_ce_and_other_exceptions_preserve_it() {
        let mut cp0 = cp0(SyntheticCp0State::new(false));
        let location = ExceptionLocation::from_pc_state(&PcState::new(0x1000));

        cp0.apply_exception(
            ExceptionRequest::CoprocessorUnusable { coprocessor: 2 },
            location,
        );
        assert_eq!(cp0.coprocessor_error(), 2);
        assert_eq!(cp0.exception_code(), ExceptionCode::CoprocessorUnusable);

        cp0.apply_exception(ExceptionRequest::Syscall, location);
        assert_eq!(cp0.coprocessor_error(), 2);
        assert_eq!(cp0.exception_code(), ExceptionCode::Syscall);
    }
}
