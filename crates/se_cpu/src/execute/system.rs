//! Maps `SYSCALL` and `BREAK` to synchronous guest exception requests.
//!
//! Encoded software code fields remain part of the typed instruction, but handlers
//! only return exception requests. They neither mutate `CP0` nor map guest `BREAK`
//! to host debugger behavior.

use crate::exception::ExceptionRequest;
use crate::execute::InstructionOutcome;

pub(super) const fn execute_syscall(_code: u32) -> InstructionOutcome {
    InstructionOutcome::Exception(ExceptionRequest::Syscall)
}

pub(super) const fn execute_break(_code: u32) -> InstructionOutcome {
    InstructionOutcome::Exception(ExceptionRequest::Breakpoint)
}

#[cfg(test)]
mod tests {
    use super::{execute_break, execute_syscall};
    use crate::exception::ExceptionRequest;
    use crate::execute::InstructionOutcome;

    #[test]
    fn syscall_requests_a_guest_system_call_exception() {
        assert_eq!(
            execute_syscall(0xabcde),
            InstructionOutcome::Exception(ExceptionRequest::Syscall)
        );
    }

    #[test]
    fn break_requests_a_guest_breakpoint_exception() {
        assert_eq!(
            execute_break(0x54321),
            InstructionOutcome::Exception(ExceptionRequest::Breakpoint)
        );
    }
}
