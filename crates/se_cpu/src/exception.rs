//! Defines guest exception reasons and their precise instruction locations.
//!
//! A request identifies the guest event without selecting an exception vector or
//! modifying exception-control state. [`ExceptionLocation`] captures the precise
//! fault and branch-origin addresses before exception entry mutates the PC.

use crate::pc::PcState;

/// Identifies an architectural `Cause.ExcCode` class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExceptionCode {
    AddressErrorLoad,
    AddressErrorStore,
    Syscall,
    Breakpoint,
    ReservedInstruction,
    IntegerOverflow,
}

/// Describes why normal retirement is replaced by guest exception entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExceptionRequest {
    IntegerOverflow,
    Syscall,
    Breakpoint,
    ReservedInstruction,
    AddressErrorLoad { bad_vaddr: u64 },
    AddressErrorStore { bad_vaddr: u64 },
}

impl ExceptionRequest {
    pub(crate) const fn exception_code(self) -> ExceptionCode {
        match self {
            Self::IntegerOverflow => ExceptionCode::IntegerOverflow,
            Self::Syscall => ExceptionCode::Syscall,
            Self::Breakpoint => ExceptionCode::Breakpoint,
            Self::ReservedInstruction => ExceptionCode::ReservedInstruction,
            Self::AddressErrorLoad { .. } => ExceptionCode::AddressErrorLoad,
            Self::AddressErrorStore { .. } => ExceptionCode::AddressErrorStore,
        }
    }

    pub(crate) const fn bad_vaddr(self) -> Option<u64> {
        match self {
            Self::AddressErrorLoad { bad_vaddr } | Self::AddressErrorStore { bad_vaddr } => {
                Some(bad_vaddr)
            }
            Self::IntegerOverflow
            | Self::Syscall
            | Self::Breakpoint
            | Self::ReservedInstruction => None,
        }
    }
}

/// Captures the faulting instruction address and authoritative delay-slot origin.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ExceptionLocation {
    fault_pc: u64,
    branch_pc: Option<u64>,
}

impl ExceptionLocation {
    /// Captures exception location before any program-counter mutation.
    pub(crate) const fn from_pc_state(pc: &PcState) -> Self {
        Self {
            fault_pc: pc.current(),
            branch_pc: pc.delay_slot_of(),
        }
    }

    /// Returns the architectural `(epc, branch_delay)` pair.
    ///
    /// A recorded branch origin supplies `epc`; otherwise the faulting instruction
    /// address does. The branch origin is never reconstructed by subtracting from
    /// that address.
    pub(crate) const fn exception_program_counter(self) -> (u64, bool) {
        match self.branch_pc {
            Some(branch_pc) => (branch_pc, true),
            None => (self.fault_pc, false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ExceptionCode, ExceptionLocation, ExceptionRequest};
    use crate::pc::{PcEffect, PcState};

    #[test]
    fn exception_requests_centralize_cause_and_address_information() {
        let cases = [
            (
                ExceptionRequest::IntegerOverflow,
                ExceptionCode::IntegerOverflow,
                None,
            ),
            (ExceptionRequest::Syscall, ExceptionCode::Syscall, None),
            (
                ExceptionRequest::Breakpoint,
                ExceptionCode::Breakpoint,
                None,
            ),
            (
                ExceptionRequest::ReservedInstruction,
                ExceptionCode::ReservedInstruction,
                None,
            ),
            (
                ExceptionRequest::AddressErrorLoad { bad_vaddr: 0x123 },
                ExceptionCode::AddressErrorLoad,
                Some(0x123),
            ),
            (
                ExceptionRequest::AddressErrorStore { bad_vaddr: 0x456 },
                ExceptionCode::AddressErrorStore,
                Some(0x456),
            ),
        ];

        for (request, code, bad_vaddr) in cases {
            assert_eq!(request.exception_code(), code);
            assert_eq!(request.bad_vaddr(), bad_vaddr);
        }
    }

    #[test]
    fn location_uses_current_pc_outside_a_delay_slot() {
        let location = ExceptionLocation::from_pc_state(&PcState::new(0x1004));

        assert_eq!(location.exception_program_counter(), (0x1004, false));
    }

    #[test]
    fn location_retains_the_authoritative_branch_origin() {
        let mut pc = PcState::new(0x1000);
        pc.apply(PcEffect::DelayedTransfer {
            after_delay_slot: 0x4000,
        });
        let location = ExceptionLocation::from_pc_state(&pc);

        assert_eq!(pc.current(), 0x1004);
        assert_eq!(location.exception_program_counter(), (0x1000, true));
    }
}
