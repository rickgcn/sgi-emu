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

    pub(super) fn pc(&self) -> u32 {
        self.pc
    }

    pub(super) fn read_gpr(&self, index: usize) -> u32 {
        if index == 0 { 0 } else { self.gpr[index] }
    }

    pub(super) fn write_gpr(&mut self, index: usize, value: u32) {
        if index != 0 {
            self.gpr[index] = value;
        }
    }

    pub(super) fn advance_pc(&mut self) {
        self.pc = self.pc.wrapping_add(4);
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

    #[test]
    fn general_register_access_preserves_register_zero() {
        let mut state = State::new();

        state.write_gpr(1, 0x1234_5678);
        state.write_gpr(31, 0x89ab_cdef);
        state.write_gpr(0, u32::MAX);

        assert_eq!(state.read_gpr(0), 0);
        assert_eq!(state.read_gpr(1), 0x1234_5678);
        assert_eq!(state.read_gpr(31), 0x89ab_cdef);
        assert_eq!(state.gpr[0], 0);
    }

    #[test]
    fn program_counter_advances_with_wrapping_arithmetic() {
        let mut state = State::new();

        assert_eq!(state.pc(), RESET_PC);
        state.advance_pc();
        assert_eq!(state.pc(), RESET_PC + 4);

        state.pc = 0xffff_fffc;
        state.advance_pc();
        assert_eq!(state.pc(), 0);
    }
}
