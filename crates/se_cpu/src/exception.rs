//! Defines guest exception reasons and their precise instruction locations.
//!
//! A request identifies the guest event without selecting an exception vector or
//! modifying exception-control state. [`ExceptionLocation`] captures the precise
//! current and branch-origin addresses before exception entry mutates the PC.

use crate::pc::PcState;

/// Identifies an architectural `Cause.ExcCode` class.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExceptionCode {
    Interrupt = 0,
    AddressErrorLoad = 4,
    AddressErrorStore = 5,
    InstructionBusError = 6,
    DataBusError = 7,
    Syscall = 8,
    Breakpoint = 9,
    ReservedInstruction = 10,
    CoprocessorUnusable = 11,
    IntegerOverflow = 12,
}

impl ExceptionCode {
    pub(crate) const fn raw(self) -> u8 {
        self as u8
    }
}

/// Describes why normal retirement is replaced by guest exception entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExceptionRequest {
    Interrupt,
    IntegerOverflow,
    Syscall,
    Breakpoint,
    ReservedInstruction,
    InstructionBusError,
    DataBusError,
    AddressErrorLoad { bad_vaddr: u64 },
    AddressErrorStore { bad_vaddr: u64 },
    CoprocessorUnusable { coprocessor: u8 },
}

impl ExceptionRequest {
    pub(crate) const fn exception_code(self) -> ExceptionCode {
        match self {
            Self::Interrupt => ExceptionCode::Interrupt,
            Self::IntegerOverflow => ExceptionCode::IntegerOverflow,
            Self::Syscall => ExceptionCode::Syscall,
            Self::Breakpoint => ExceptionCode::Breakpoint,
            Self::ReservedInstruction => ExceptionCode::ReservedInstruction,
            Self::InstructionBusError => ExceptionCode::InstructionBusError,
            Self::DataBusError => ExceptionCode::DataBusError,
            Self::AddressErrorLoad { .. } => ExceptionCode::AddressErrorLoad,
            Self::AddressErrorStore { .. } => ExceptionCode::AddressErrorStore,
            Self::CoprocessorUnusable { .. } => ExceptionCode::CoprocessorUnusable,
        }
    }

    pub(crate) const fn bad_vaddr(self) -> Option<u64> {
        match self {
            Self::AddressErrorLoad { bad_vaddr } | Self::AddressErrorStore { bad_vaddr } => {
                Some(bad_vaddr)
            }
            Self::IntegerOverflow
            | Self::Interrupt
            | Self::Syscall
            | Self::Breakpoint
            | Self::ReservedInstruction
            | Self::InstructionBusError
            | Self::DataBusError
            | Self::CoprocessorUnusable { .. } => None,
        }
    }

    pub(crate) const fn coprocessor(self) -> Option<u8> {
        match self {
            Self::CoprocessorUnusable { coprocessor } => Some(coprocessor),
            Self::Interrupt
            | Self::IntegerOverflow
            | Self::Syscall
            | Self::Breakpoint
            | Self::ReservedInstruction
            | Self::InstructionBusError
            | Self::DataBusError
            | Self::AddressErrorLoad { .. }
            | Self::AddressErrorStore { .. } => None,
        }
    }
}

/// Captures the current instruction address and authoritative delay-slot origin.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ExceptionLocation {
    current_pc: u64,
    branch_pc: Option<u64>,
}

impl ExceptionLocation {
    /// Captures exception location before any program-counter mutation.
    pub(crate) const fn from_pc_state(pc: &PcState) -> Self {
        Self {
            current_pc: pc.current(),
            branch_pc: pc.delay_slot_of(),
        }
    }

    /// Returns the architectural `(epc, branch_delay)` pair.
    ///
    /// A recorded branch origin supplies `epc`; otherwise the current instruction
    /// address does. The branch origin is never reconstructed by subtracting from
    /// that address.
    pub(crate) const fn exception_program_counter(self) -> (u64, bool) {
        match self.branch_pc {
            Some(branch_pc) => (branch_pc, true),
            None => (self.current_pc, false),
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
            (ExceptionRequest::Interrupt, ExceptionCode::Interrupt, None),
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
                ExceptionRequest::InstructionBusError,
                ExceptionCode::InstructionBusError,
                None,
            ),
            (
                ExceptionRequest::DataBusError,
                ExceptionCode::DataBusError,
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
            (
                ExceptionRequest::CoprocessorUnusable { coprocessor: 2 },
                ExceptionCode::CoprocessorUnusable,
                None,
            ),
        ];

        for (request, code, bad_vaddr) in cases {
            assert_eq!(request.exception_code(), code);
            assert_eq!(request.bad_vaddr(), bad_vaddr);
        }

        assert_eq!(
            ExceptionRequest::CoprocessorUnusable { coprocessor: 2 }.coprocessor(),
            Some(2)
        );
        assert_eq!(ExceptionRequest::Interrupt.coprocessor(), None);
    }

    #[test]
    fn cause_codes_match_the_r10000_architectural_encoding() {
        assert_eq!(ExceptionCode::Interrupt.raw(), 0);
        assert_eq!(ExceptionCode::AddressErrorLoad.raw(), 4);
        assert_eq!(ExceptionCode::AddressErrorStore.raw(), 5);
        assert_eq!(ExceptionCode::InstructionBusError.raw(), 6);
        assert_eq!(ExceptionCode::DataBusError.raw(), 7);
        assert_eq!(ExceptionCode::Syscall.raw(), 8);
        assert_eq!(ExceptionCode::Breakpoint.raw(), 9);
        assert_eq!(ExceptionCode::ReservedInstruction.raw(), 10);
        assert_eq!(ExceptionCode::CoprocessorUnusable.raw(), 11);
        assert_eq!(ExceptionCode::IntegerOverflow.raw(), 12);
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
