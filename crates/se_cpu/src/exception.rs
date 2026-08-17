//! Separates guest architectural exception requests from emulator failures.
//!
//! A request identifies the guest event without selecting an exception vector or
//! modifying exception-control state.

/// Identifies a guest exception independently of exception-state commit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExceptionRequest {
    /// The raw word was positively classified as an architecturally reserved encoding.
    ReservedInstruction,
}
