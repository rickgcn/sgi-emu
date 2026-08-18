//! Applies the CP0 state transitions required for translation, exception entry, and return.
//!
//! [`Cp0`] separates TLB-write staging registers from the authoritative TLB.
//! Exception requests supply the cause and optional fault address, while
//! exception locations supply the precise `EPC` and `BD` context. TLB exceptions
//! update their diagnostic virtual-page fields independently of protected
//! `EPC`/`BD` state. External hardware interrupt pending bits remain live inputs
//! rather than stored Cause state. The module exposes no architectural reset
//! values.

use crate::exception::{ExceptionCode, ExceptionLocation, ExceptionRequest};
use crate::tlb::{TlbFault, TlbFaultReason};

const NORMAL_GENERAL_EXCEPTION_VECTOR: u64 = 0xffff_ffff_8000_0180;
const BOOTSTRAP_GENERAL_EXCEPTION_VECTOR: u64 = 0xffff_ffff_bfc0_0380;
const NORMAL_TLB_REFILL_VECTOR: u64 = 0xffff_ffff_8000_0000;
const BOOTSTRAP_TLB_REFILL_VECTOR: u64 = 0xffff_ffff_bfc0_0200;
const VPN2_MASK: u32 = 0x0007_ffff;
const PFN_MASK: u32 = 0x0fff_ffff;
const CONTEXT_PTE_BASE_MASK: u64 = 0xffff_ffff_ff80_0000;
const CONTEXT_BAD_VPN2_MASK: u32 = 0x0007_ffff;
const XCONTEXT_PTE_BASE_MASK: u64 = 0xffff_ffe0_0000_0000;
const XCONTEXT_BAD_VPN2_MASK: u32 = 0x7fff_ffff;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Status {
    interrupt_enable: bool,
    exl: bool,
    erl: bool,
    interrupt_mask: u8,
    operating_mode: OperatingMode,
    cu0: bool,
    bev: bool,
    tlb_shutdown: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Cause {
    exception_code: ExceptionCode,
    branch_delay: bool,
    coprocessor_error: u8,
}

/// Holds the staged TLB virtual tag and current address-space identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EntryHi {
    vpn2: u32,
    asid: u8,
}

impl EntryHi {
    pub(crate) const fn vpn2(self) -> u32 {
        self.vpn2
    }

    pub(crate) const fn asid(self) -> u8 {
        self.asid
    }
}

/// Holds one even or odd TLB page's staged translation fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EntryLo {
    pfn: u32,
    valid: bool,
    dirty: bool,
    global: bool,
}

impl EntryLo {
    const fn from_mtc0(value: u64) -> Self {
        Self {
            pfn: ((value >> 6) as u32) & PFN_MASK,
            valid: value & (1 << 1) != 0,
            dirty: value & (1 << 2) != 0,
            global: value & 1 != 0,
        }
    }

    pub(crate) const fn pfn(self) -> u32 {
        self.pfn
    }

    pub(crate) const fn valid(self) -> bool {
        self.valid
    }

    pub(crate) const fn dirty(self) -> bool {
        self.dirty
    }

    pub(crate) const fn global(self) -> bool {
        self.global
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Context {
    pte_base: u64,
    bad_vpn2: u32,
}

impl Context {
    const fn encoded(self) -> u64 {
        (self.pte_base & CONTEXT_PTE_BASE_MASK)
            | (((self.bad_vpn2 & CONTEXT_BAD_VPN2_MASK) as u64) << 4)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct XContext {
    pte_base: u64,
    bad_vpn2: u32,
}

impl XContext {
    const fn encoded(self) -> u64 {
        let bad_vpn2 = self.bad_vpn2 & XCONTEXT_BAD_VPN2_MASK;
        // Canonical 32-bit addresses make the region field derivable from the
        // sign carried in the high BadVPN2 bits: either user region 00 or kernel
        // region 11.
        let region = if bad_vpn2 & (1 << 30) == 0 { 0 } else { 3 };
        (self.pte_base & XCONTEXT_PTE_BASE_MASK) | (region << 35) | ((bad_vpn2 as u64) << 4)
    }
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
    /// Replaces the six-bit indexed-write target.
    WriteIndex { index: u8 },
    /// Replaces the staged even-page translation fields.
    WriteEntryLo0 { entry: EntryLo },
    /// Replaces the staged odd-page translation fields.
    WriteEntryLo1 { entry: EntryLo },
    /// Records whether the indexed TLB write detected a conflicting old entry.
    SetTlbShutdown { value: bool },
}

impl Cp0Effect {
    /// Constructs an `Index` write from the source value's low six bits.
    pub(crate) const fn write_index(value: u64) -> Self {
        Self::WriteIndex {
            index: (value as u8) & 0x3f,
        }
    }

    /// Constructs an `EntryLo0` write with `PFN` from bits 33:6.
    ///
    /// `D`, `V`, and `G` come from bits 2, 1, and 0 of the full 64-bit `MTC0`
    /// source value.
    pub(crate) const fn write_entry_lo0(value: u64) -> Self {
        Self::WriteEntryLo0 {
            entry: EntryLo::from_mtc0(value),
        }
    }

    /// Constructs an `EntryLo1` write with `PFN` from bits 33:6.
    ///
    /// `D`, `V`, and `G` come from bits 2, 1, and 0 of the full 64-bit `MTC0`
    /// source value.
    pub(crate) const fn write_entry_lo1(value: u64) -> Self {
        Self::WriteEntryLo1 {
            entry: EntryLo::from_mtc0(value),
        }
    }

    pub(crate) const fn set_tlb_shutdown(value: bool) -> Self {
        Self::SetTlbShutdown { value }
    }
}

/// Stores the CP0 subset used by address translation, exception handling, and
/// supported moves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Cp0 {
    status: Status,
    cause: Cause,
    index: u8,
    entry_hi: EntryHi,
    entry_lo0: EntryLo,
    entry_lo1: EntryLo,
    context: Context,
    xcontext: XContext,
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
    entry_hi_asid: u8,
    context_pte_base: u64,
    xcontext_pte_base: u64,
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
            entry_hi_asid: 0,
            context_pte_base: 0,
            xcontext_pte_base: 0,
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

    pub(crate) const fn with_entry_hi_asid(mut self, asid: u8) -> Self {
        self.entry_hi_asid = asid;
        self
    }

    pub(crate) const fn with_context_pte_base(mut self, pte_base: u64) -> Self {
        self.context_pte_base = pte_base;
        self
    }

    pub(crate) const fn with_xcontext_pte_base(mut self, pte_base: u64) -> Self {
        self.xcontext_pte_base = pte_base;
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
                tlb_shutdown: false,
            },
            cause: Cause {
                exception_code: ExceptionCode::ReservedInstruction,
                branch_delay: false,
                coprocessor_error: 0,
            },
            index: 0,
            entry_hi: EntryHi {
                vpn2: 0,
                asid: state.entry_hi_asid,
            },
            entry_lo0: EntryLo::from_mtc0(0),
            entry_lo1: EntryLo::from_mtc0(0),
            context: Context {
                pte_base: state.context_pte_base & CONTEXT_PTE_BASE_MASK,
                bad_vpn2: 0,
            },
            xcontext: XContext {
                pte_base: state.xcontext_pte_base & XCONTEXT_PTE_BASE_MASK,
                bad_vpn2: 0,
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

    pub(crate) const fn tlb_shutdown(&self) -> bool {
        self.status.tlb_shutdown
    }

    pub(crate) const fn index(&self) -> u8 {
        self.index
    }

    pub(crate) const fn entry_hi(&self) -> EntryHi {
        self.entry_hi
    }

    pub(crate) const fn entry_lo0(&self) -> EntryLo {
        self.entry_lo0
    }

    pub(crate) const fn entry_lo1(&self) -> EntryLo {
        self.entry_lo1
    }

    /// Returns the sign-extended low word read by `MFC0 Context`.
    pub(crate) const fn mfc0_context(&self) -> u64 {
        self.context.encoded() as u32 as i32 as i64 as u64
    }

    #[cfg(test)]
    pub(crate) const fn context(&self) -> u64 {
        self.context.encoded()
    }

    #[cfg(test)]
    pub(crate) const fn xcontext(&self) -> u64 {
        self.xcontext.encoded()
    }

    #[cfg(test)]
    pub(crate) const fn context_bad_vpn2(&self) -> u32 {
        self.context.bad_vpn2
    }

    #[cfg(test)]
    pub(crate) const fn xcontext_bad_vpn2(&self) -> u32 {
        self.xcontext.bad_vpn2
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

    /// Applies one exception and returns its selected exception vector.
    ///
    /// When `Status.EXL` is clear, this captures `EPC` and `Cause.BD` from
    /// `location`; when it is set, both fields remain protected. Every request
    /// updates `Cause.ExcCode` and sets `Status.EXL`. A Coprocessor Unusable
    /// request updates `Cause.CE`; other requests preserve its previous value as a
    /// deterministic policy because the architecture leaves it undefined. A
    /// request carrying a fault address writes `BadVAddr`. A TLB request also
    /// updates the `Context`, `XContext`, and `EntryHi` diagnostic page fields
    /// while preserving `EntryHi.ASID` and both `EntryLo` staging registers. For
    /// other requests this implementation leaves the stored address diagnostics
    /// untouched where the architecture does not define them.
    ///
    /// A refill observed with clear pre-exception `Status.EXL` selects
    /// `0xffff_ffff_8000_0000`, or `0xffff_ffff_bfc0_0200` when `Status.BEV` is
    /// set. Invalid, modified, and nested refill exceptions select the respective
    /// general vector at `0xffff_ffff_8000_0180` or `0xffff_ffff_bfc0_0380`.
    pub(crate) fn apply_exception(
        &mut self,
        request: ExceptionRequest,
        location: ExceptionLocation,
    ) -> u64 {
        let pre_exl = self.status.exl;
        let vector = self.exception_vector(request, pre_exl);

        self.cause.exception_code = request.exception_code();
        if let Some(coprocessor) = request.coprocessor() {
            self.cause.coprocessor_error = coprocessor;
        }
        if !pre_exl {
            let (epc, branch_delay) = location.exception_program_counter();
            self.epc = epc;
            self.cause.branch_delay = branch_delay;
        }
        // The R10000 architecture leaves BadVAddr undefined for non-address
        // exceptions. Preserve its previous value as the deterministic policy.
        if let Some(bad_vaddr) = request.bad_vaddr() {
            self.bad_vaddr = bad_vaddr;
        }
        if let Some(fault) = request.tlb_fault() {
            self.apply_tlb_diagnostics(fault);
        }
        self.status.exl = true;

        vector
    }

    /// Applies one commit-carried CP0 effect without re-reading execution pre-state.
    pub(crate) fn apply_effect(&mut self, effect: Cp0Effect) {
        match effect {
            Cp0Effect::ExceptionReturn {
                level: ExceptionReturnLevel::Exception,
            } => self.status.exl = false,
            Cp0Effect::ExceptionReturn {
                level: ExceptionReturnLevel::Error,
            } => self.status.erl = false,
            Cp0Effect::WriteIndex { index } => self.index = index,
            Cp0Effect::WriteEntryLo0 { entry } => self.entry_lo0 = entry,
            Cp0Effect::WriteEntryLo1 { entry } => self.entry_lo1 = entry,
            Cp0Effect::SetTlbShutdown { value } => self.status.tlb_shutdown = value,
        }
    }

    fn apply_tlb_diagnostics(&mut self, fault: TlbFault) {
        let virtual_address = fault.virtual_address();
        self.context.bad_vpn2 = ((virtual_address >> 13) as u32) & CONTEXT_BAD_VPN2_MASK;
        self.xcontext.bad_vpn2 = ((virtual_address >> 13) as u32) & XCONTEXT_BAD_VPN2_MASK;
        self.entry_hi.vpn2 = ((virtual_address as u32) >> 13) & VPN2_MASK;
    }

    fn exception_vector(&self, request: ExceptionRequest, pre_exl: bool) -> u64 {
        if !pre_exl
            && request
                .tlb_fault()
                .is_some_and(|fault| fault.reason() == TlbFaultReason::Refill)
        {
            self.tlb_refill_vector()
        } else {
            self.general_exception_vector()
        }
    }

    const fn tlb_refill_vector(&self) -> u64 {
        if self.status.bev {
            BOOTSTRAP_TLB_REFILL_VECTOR
        } else {
            NORMAL_TLB_REFILL_VECTOR
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
    use super::{Cp0, Cp0Effect, ExceptionReturnLevel, OperatingMode, SyntheticCp0State};
    use crate::exception::{ExceptionCode, ExceptionLocation, ExceptionRequest};
    use crate::memory::AccessKind;
    use crate::pc::PcState;
    use crate::tlb::{TlbFault, TlbFaultReason};

    fn cp0(state: SyntheticCp0State) -> Cp0 {
        Cp0::synthetic_test_state_with(state)
    }

    fn tlb_request(
        reason: TlbFaultReason,
        access: AccessKind,
        virtual_address: u64,
    ) -> ExceptionRequest {
        ExceptionRequest::Tlb(TlbFault::new_for_test(reason, access, virtual_address))
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

    #[test]
    fn tlb_exception_updates_page_diagnostics_and_preserves_staging() {
        const VIRTUAL_ADDRESS: u64 = 0xffff_ffff_e123_4567;
        let mut cp0 = cp0(SyntheticCp0State::new(false)
            .with_entry_hi_asid(0x5a)
            .with_context_pte_base(0xffff_ffff_a000_0000)
            .with_xcontext_pte_base(0x1234_5600_0000_0000));
        cp0.apply_effect(Cp0Effect::write_entry_lo0(
            (0x0abc_def0_u64 << 6) | (1 << 62) | 0x3f,
        ));
        cp0.apply_effect(Cp0Effect::write_entry_lo1(
            (0x0123_4567_u64 << 6) | (1 << 61) | (1 << 1),
        ));
        let location = ExceptionLocation::from_pc_state(&PcState::new(0x1000));

        let vector = cp0.apply_exception(
            tlb_request(TlbFaultReason::Refill, AccessKind::Load, VIRTUAL_ADDRESS),
            location,
        );

        assert_eq!(vector, 0xffff_ffff_8000_0000);
        assert_eq!(cp0.bad_vaddr(), VIRTUAL_ADDRESS);
        assert_eq!(cp0.context_bad_vpn2(), 0x0007_091a);
        assert_eq!(cp0.xcontext_bad_vpn2(), 0x7fff_091a);
        assert_eq!(cp0.context(), 0xffff_ffff_a070_91a0);
        assert_eq!(cp0.xcontext(), 0x1234_561f_fff0_91a0);
        assert_eq!(cp0.mfc0_context(), 0xffff_ffff_a070_91a0);
        assert_eq!(cp0.entry_hi().vpn2(), 0x0007_091a);
        assert_eq!(cp0.entry_hi().asid(), 0x5a);
        assert_eq!(cp0.entry_lo0().pfn(), 0x0abc_def0);
        assert!(cp0.entry_lo0().valid());
        assert!(cp0.entry_lo0().dirty());
        assert!(cp0.entry_lo0().global());
        assert_eq!(cp0.entry_lo1().pfn(), 0x0123_4567);
        assert!(cp0.entry_lo1().valid());
        assert!(!cp0.entry_lo1().dirty());
        assert!(!cp0.entry_lo1().global());
    }

    #[test]
    fn refill_vector_requires_clear_pre_exception_exl() {
        let location = ExceptionLocation::from_pc_state(&PcState::new(0x2000));
        for (bev, expected) in [
            (false, 0xffff_ffff_8000_0000),
            (true, 0xffff_ffff_bfc0_0200),
        ] {
            let mut cp0 = cp0(SyntheticCp0State::new(bev));
            assert_eq!(
                cp0.apply_exception(
                    tlb_request(TlbFaultReason::Refill, AccessKind::Fetch, 0x4000),
                    location,
                ),
                expected
            );
        }

        for request in [
            tlb_request(TlbFaultReason::Invalid, AccessKind::Load, 0x4000),
            tlb_request(TlbFaultReason::Modified, AccessKind::Store, 0x4000),
        ] {
            let mut cp0 = cp0(SyntheticCp0State::new(false));
            assert_eq!(
                cp0.apply_exception(request, location),
                0xffff_ffff_8000_0180
            );
        }

        let mut nested = cp0(SyntheticCp0State::new(false)
            .with_exception_levels(true, false)
            .with_return_addresses(0x1234, 0));
        assert_eq!(
            nested.apply_exception(
                tlb_request(TlbFaultReason::Refill, AccessKind::Load, 0x6000),
                location,
            ),
            0xffff_ffff_8000_0180
        );
        assert_eq!(nested.epc(), 0x1234);
        assert!(!nested.branch_delay());
        assert_eq!(nested.entry_hi().vpn2(), 3);
    }

    #[test]
    fn ordinary_address_error_preserves_tlb_page_diagnostics() {
        let location = ExceptionLocation::from_pc_state(&PcState::new(0x3000));
        let mut cp0 = cp0(SyntheticCp0State::new(false)
            .with_entry_hi_asid(9)
            .with_context_pte_base(0xffff_ffff_a000_0000)
            .with_xcontext_pte_base(0x1234_5600_0000_0000));
        cp0.apply_exception(
            tlb_request(TlbFaultReason::Refill, AccessKind::Load, 0x0040_1000),
            location,
        );
        let context = cp0.context();
        let xcontext = cp0.xcontext();
        let entry_hi = cp0.entry_hi();

        cp0.apply_exception(
            ExceptionRequest::AddressErrorStore {
                bad_vaddr: 0x0000_0000_8000_0000,
            },
            location,
        );

        assert_eq!(cp0.bad_vaddr(), 0x0000_0000_8000_0000);
        assert_eq!(cp0.context(), context);
        assert_eq!(cp0.xcontext(), xcontext);
        assert_eq!(cp0.entry_hi(), entry_hi);
    }
}
