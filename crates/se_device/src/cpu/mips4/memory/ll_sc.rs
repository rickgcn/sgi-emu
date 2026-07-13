//! LL/SC atomicity state machine.
//!
//! This module models the `LLbit` virtual state used by the linked load and
//! store conditional instructions `LL`/`LLD`/`SC`/`SCD` (MIPS IV manual section
//! A.5). It only describes state transitions; it does not perform memory
//! accesses, manage cache coherence, or deliver exceptions.
//!
//! Manual semantics: `LLbit` is set when a linked load occurs, tested and
//! cleared by the conditional store, and cleared when a store to the location
//! would no longer be atomic (including by exception return instructions).

/// Value of the `LLbit` virtual state.
#[derive(
    Clone, Copy, Debug, Default, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize,
)]
pub enum Mips4LlBit {
    /// No active read-modify-write sequence.
    #[default]
    Clear,

    /// A linked load has begun a read-modify-write sequence.
    Set,
}

/// An event that transitions `LLbit`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Mips4LlBitEvent {
    /// A linked load (`LL`/`LLD`) begins a read-modify-write sequence and sets `LLbit`.
    LinkedLoad,

    /// A store conditional (`SC`/`SCD`) tests `LLbit`, clears it, and reports success.
    StoreConditional,

    /// An invalidating event clears `LLbit` because the store would no longer be
    /// atomic (for example an exception return or an external coherent store).
    Invalidate,
}

/// Result of an `LLbit` transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Mips4LlBitTransition {
    /// New `LLbit` value after the event.
    pub state: Mips4LlBit,

    /// Whether a store conditional succeeded, when the event was
    /// [`Mips4LlBitEvent::StoreConditional`]. The value is the `LLbit` value
    /// before the store (matching `GPR[rt] <- 0^63 || LLbit`); `None` for other
    /// events.
    pub store_conditional_succeeded: Option<bool>,
}

impl Mips4LlBit {
    /// Applies an event and returns the resulting state and store-conditional outcome.
    pub const fn transition(self, event: Mips4LlBitEvent) -> Mips4LlBitTransition {
        match event {
            Mips4LlBitEvent::LinkedLoad => Mips4LlBitTransition {
                state: Mips4LlBit::Set,
                store_conditional_succeeded: None,
            },
            Mips4LlBitEvent::StoreConditional => Mips4LlBitTransition {
                state: Mips4LlBit::Clear,
                store_conditional_succeeded: Some(matches!(self, Mips4LlBit::Set)),
            },
            Mips4LlBitEvent::Invalidate => Mips4LlBitTransition {
                state: Mips4LlBit::Clear,
                store_conditional_succeeded: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linked_load_sets_the_bit_regardless_of_prior_state() {
        assert_eq!(
            Mips4LlBit::Clear.transition(Mips4LlBitEvent::LinkedLoad),
            Mips4LlBitTransition {
                state: Mips4LlBit::Set,
                store_conditional_succeeded: None,
            }
        );
        assert_eq!(
            Mips4LlBit::Set.transition(Mips4LlBitEvent::LinkedLoad),
            Mips4LlBitTransition {
                state: Mips4LlBit::Set,
                store_conditional_succeeded: None,
            }
        );
    }

    #[test]
    fn store_conditional_tests_then_clears_and_reports_success() {
        assert_eq!(
            Mips4LlBit::Set.transition(Mips4LlBitEvent::StoreConditional),
            Mips4LlBitTransition {
                state: Mips4LlBit::Clear,
                store_conditional_succeeded: Some(true),
            }
        );
        assert_eq!(
            Mips4LlBit::Clear.transition(Mips4LlBitEvent::StoreConditional),
            Mips4LlBitTransition {
                state: Mips4LlBit::Clear,
                store_conditional_succeeded: Some(false),
            }
        );
    }

    #[test]
    fn invalidate_clears_the_bit_from_either_state() {
        assert_eq!(
            Mips4LlBit::Set.transition(Mips4LlBitEvent::Invalidate),
            Mips4LlBitTransition {
                state: Mips4LlBit::Clear,
                store_conditional_succeeded: None,
            }
        );
        assert_eq!(
            Mips4LlBit::Clear.transition(Mips4LlBitEvent::Invalidate),
            Mips4LlBitTransition {
                state: Mips4LlBit::Clear,
                store_conditional_succeeded: None,
            }
        );
    }

    #[test]
    fn default_state_is_clear() {
        assert_eq!(Mips4LlBit::default(), Mips4LlBit::Clear);
    }
}
