const RESET_PC: u32 = 0xbfc0_0000;

pub(super) struct State {
    gpr: [u32; 32],
    pc: u32,
}

impl State {
    pub(super) fn new() -> Self {
        Self {
            gpr: [0; 32],
            pc: RESET_PC,
        }
    }

    pub(super) fn reset(&mut self) {
        self.gpr[0] = 0;
        self.pc = RESET_PC;
    }
}

#[cfg(test)]
mod tests {
    use super::{RESET_PC, State};

    #[test]
    fn new_initializes_deterministic_state() {
        let state = State::new();

        assert_eq!(state.gpr, [0; 32]);
        assert_eq!(state.pc, RESET_PC);
    }

    #[test]
    fn reset_restores_defined_state_only() {
        let mut state = State::new();
        for (index, register) in state.gpr.iter_mut().enumerate() {
            *register = index as u32 + 1;
        }
        state.pc = 0;
        let preserved_gpr = state.gpr;

        state.reset();

        assert_eq!(state.gpr[0], 0);
        assert_eq!(state.gpr[1..], preserved_gpr[1..]);
        assert_eq!(state.pc, RESET_PC);
    }
}
