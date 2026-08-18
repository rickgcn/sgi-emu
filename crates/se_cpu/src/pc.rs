//! Centralizes normal and exception program-counter transitions.
//!
//! [`PcState`] owns the current address, selected successor, and delay-slot origin.
//! For normal retirement, instruction handlers supply a [`PcEffect`]; branch
//! conditions and targets are resolved before this module mutates state.
//! Exception entry and exception return bypass [`PcEffect`] and replace all
//! control-flow state with a selected address and its sequential successor.

/// Describes the program-counter state change of a normal retirement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PcEffect {
    /// Selects the previous `next` address and clears delay-slot context.
    Sequential,
    /// Advances into the sequential delay slot and selects its continuation.
    DelayedTransfer {
        /// Address selected after the delay slot retires normally.
        after_delay_slot: u64,
    },
}

/// Tracks current and next instruction addresses together with delay-slot origin.
///
/// In sequential state, `next` is `current + 4` and `delay_slot_of` is `None`. In
/// delay-slot state, `current` names the slot, `next` names its continuation, and
/// `delay_slot_of` names the transfer that created the slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PcState {
    current: u64,
    next: u64,
    delay_slot_of: Option<u64>,
}

impl PcState {
    pub(crate) const fn new(current: u64) -> Self {
        Self {
            current,
            next: current.wrapping_add(4),
            delay_slot_of: None,
        }
    }

    pub(crate) const fn current(&self) -> u64 {
        self.current
    }

    pub(crate) const fn next(&self) -> u64 {
        self.next
    }

    pub(crate) const fn delay_slot_of(&self) -> Option<u64> {
        self.delay_slot_of
    }

    pub(crate) const fn is_delay_slot(&self) -> bool {
        self.delay_slot_of.is_some()
    }

    /// Applies a program-counter effect for normal retirement.
    ///
    /// # Panics
    ///
    /// Panics if [`PcEffect::DelayedTransfer`] is applied while the current
    /// instruction already occupies a delay slot.
    pub(crate) fn apply(&mut self, effect: PcEffect) {
        match effect {
            PcEffect::Sequential => {
                self.current = self.next;
                self.next = self.next.wrapping_add(4);
                self.delay_slot_of = None;
            }
            PcEffect::DelayedTransfer { after_delay_slot } => {
                assert!(
                    self.delay_slot_of.is_none(),
                    "a delayed transfer commit cannot originate in a delay slot"
                );
                let origin = self.current;
                self.current = self.next;
                self.next = after_delay_slot;
                self.delay_slot_of = Some(origin);
            }
        }
    }

    /// Replaces all control-flow state with an exception vector's sequential state.
    ///
    /// This transition does not apply a normal [`PcEffect`]. Successor arithmetic
    /// wraps, and any delay-slot origin is discarded.
    pub(crate) fn enter_exception(&mut self, vector: u64) {
        self.replace_with_sequential(vector);
    }

    /// Replaces all control-flow state with an exception-return target.
    ///
    /// Successor arithmetic wraps, and any delay-slot origin is discarded.
    pub(crate) fn return_from_exception(&mut self, target: u64) {
        self.replace_with_sequential(target);
    }

    fn replace_with_sequential(&mut self, current: u64) {
        self.current = current;
        self.next = current.wrapping_add(4);
        self.delay_slot_of = None;
    }
}

#[cfg(test)]
mod tests {
    use super::{PcEffect, PcState};

    #[test]
    fn sequential_retirement_advances_both_addresses() {
        let mut pc = PcState::new(0x1000);

        pc.apply(PcEffect::Sequential);

        assert_eq!(pc.current(), 0x1004);
        assert_eq!(pc.next(), 0x1008);
        assert_eq!(pc.delay_slot_of(), None);
    }

    #[test]
    fn delayed_transfer_enters_the_delay_slot() {
        let mut pc = PcState::new(0x1000);

        pc.apply(PcEffect::DelayedTransfer {
            after_delay_slot: 0x2000,
        });

        assert_eq!(pc.current(), 0x1004);
        assert_eq!(pc.next(), 0x2000);
        assert_eq!(pc.delay_slot_of(), Some(0x1000));
    }

    #[test]
    fn sequential_delay_slot_retirement_enters_the_continuation() {
        let mut pc = PcState::new(0x1000);
        pc.apply(PcEffect::DelayedTransfer {
            after_delay_slot: 0x2000,
        });

        pc.apply(PcEffect::Sequential);

        assert_eq!(pc.current(), 0x2000);
        assert_eq!(pc.next(), 0x2004);
        assert_eq!(pc.delay_slot_of(), None);
    }

    #[test]
    fn sequential_address_arithmetic_wraps() {
        let mut pc = PcState::new(u64::MAX - 3);

        assert_eq!(pc.next(), 0);
        pc.apply(PcEffect::Sequential);

        assert_eq!(pc.current(), 0);
        assert_eq!(pc.next(), 4);
    }

    #[test]
    fn exception_entry_replaces_all_control_flow_state() {
        let mut pc = PcState::new(0x1000);
        pc.apply(PcEffect::DelayedTransfer {
            after_delay_slot: 0x2000,
        });

        pc.enter_exception(u64::MAX - 3);

        assert_eq!(pc.current(), u64::MAX - 3);
        assert_eq!(pc.next(), 0);
        assert_eq!(pc.delay_slot_of(), None);
    }
}
