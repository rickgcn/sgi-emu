//! Runtime control state shared with frontends.

/// Current execution state of the emulator runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeState {
    /// No emulated machine has been constructed.
    Unconfigured,
}
