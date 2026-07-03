//! Machine-level events for the IP12 profile.
//!
//! These events represent board-level control transitions handled by machine
//! orchestration.

/// IP12 machine-level event payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ip12Event {
    /// Initial board power-on event.
    PowerOn,

    /// Board reset event.
    Reset,
}
