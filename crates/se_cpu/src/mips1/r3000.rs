//! MIPS R3000 processor model.

mod state;

use self::state::State;

/// An architectural R3000 processor.
pub struct R3000 {
    state: State,
}

#[expect(
    clippy::new_without_default,
    reason = "Processor construction has explicit reset semantics"
)]
impl R3000 {
    /// Creates a processor at the reset vector.
    ///
    /// General-purpose registers without architecturally defined reset values
    /// are initialized to zero.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: State::new(),
        }
    }

    /// Restores the architecturally defined core reset state.
    ///
    /// General-purpose registers other than register zero are preserved
    /// because their reset values are architecturally unspecified.
    pub fn reset(&mut self) {
        self.state.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::R3000;

    #[test]
    fn processor_can_be_constructed_and_reset() {
        let mut processor = R3000::new();

        processor.reset();
    }
}
