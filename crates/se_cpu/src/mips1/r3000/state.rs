use super::cp0::{Cp0, Exception};

const RESET_PC: u32 = 0xbfc0_0000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DelaySlot {
    origin_pc: u32,
    resume_pc: u32,
}

pub(super) struct State {
    gpr: [u32; 32],
    pc: u32,
    delay_slot: Option<DelaySlot>,
    cp0: Cp0,
}

impl State {
    pub(super) fn new() -> Self {
        Self {
            gpr: [0; 32],
            pc: RESET_PC,
            delay_slot: None,
            cp0: Cp0::new(),
        }
    }

    pub(super) fn reset(&mut self) {
        let interrupted_pc = self.pc;
        self.cp0.reset(interrupted_pc);
        self.gpr[0] = 0;
        self.pc = RESET_PC;
        self.delay_slot = None;
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

    pub(super) fn complete_instruction(&mut self, delayed_resume_pc: Option<u32>) {
        let origin_pc = self.pc;

        self.pc = match self.delay_slot.take() {
            Some(delay_slot) => delay_slot.resume_pc,
            None => self.pc.wrapping_add(4),
        };

        self.delay_slot = delayed_resume_pc.map(|resume_pc| DelaySlot {
            origin_pc,
            resume_pc,
        });
    }

    pub(super) fn take_exception(&mut self, exception: Exception) {
        let (epc, in_delay_slot) = match self.delay_slot.take() {
            Some(delay_slot) => (delay_slot.origin_pc, true),
            None => (self.pc, false),
        };

        self.pc = self.cp0.take_exception(exception, epc, in_delay_slot);
    }
}

#[cfg(test)]
mod tests {
    use super::{Cp0, DelaySlot, Exception, RESET_PC, State};

    #[test]
    fn new_initializes_deterministic_state() {
        let state = State::new();

        assert_eq!(state.gpr, [0; 32]);
        assert_eq!(state.pc, RESET_PC);
        assert_eq!(state.delay_slot, None);
        assert_eq!(state.cp0, Cp0::new());
    }

    #[test]
    fn reset_restores_defined_state_only() {
        let mut state = State::new();
        for (index, register) in state.gpr.iter_mut().enumerate() {
            *register = index as u32 + 1;
        }
        state.pc = 0;
        state.delay_slot = Some(DelaySlot {
            origin_pc: 0xffff_fff8,
            resume_pc: 0x1234_5678,
        });
        let preserved_gpr = state.gpr;
        let mut expected_cp0 = Cp0::new();
        expected_cp0.reset(state.pc);

        state.reset();

        assert_eq!(state.gpr[0], 0);
        assert_eq!(state.gpr[1..], preserved_gpr[1..]);
        assert_eq!(state.pc, RESET_PC);
        assert_eq!(state.delay_slot, None);
        assert_eq!(state.cp0, expected_cp0);
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
    fn sequential_completion_advances_with_wrapping_arithmetic() {
        let mut state = State::new();

        assert_eq!(state.pc(), RESET_PC);
        state.complete_instruction(None);
        assert_eq!(state.pc(), RESET_PC + 4);

        state.pc = 0xffff_fffc;
        state.complete_instruction(None);
        assert_eq!(state.pc(), 0);
    }

    #[test]
    fn control_flow_completion_enters_and_leaves_delay_slot() {
        let mut state = State::new();
        let resume_pc = 0xbfc0_0040;

        state.complete_instruction(Some(resume_pc));

        assert_eq!(state.pc(), RESET_PC + 4);
        assert_eq!(
            state.delay_slot,
            Some(DelaySlot {
                origin_pc: RESET_PC,
                resume_pc,
            })
        );

        state.complete_instruction(None);

        assert_eq!(state.pc(), resume_pc);
        assert_eq!(state.delay_slot, None);
    }

    #[test]
    fn not_taken_branch_still_records_delay_slot_origin() {
        let mut state = State::new();
        let fallthrough = RESET_PC + 8;

        state.complete_instruction(Some(fallthrough));

        assert_eq!(
            state.delay_slot,
            Some(DelaySlot {
                origin_pc: RESET_PC,
                resume_pc: fallthrough,
            })
        );

        state.complete_instruction(None);

        assert_eq!(state.pc(), fallthrough);
        assert_eq!(state.delay_slot, None);
    }

    #[test]
    fn delay_slot_resume_address_can_wrap_to_zero() {
        let mut state = State::new();
        state.pc = 0xffff_fffc;

        state.complete_instruction(Some(0));
        assert_eq!(state.pc(), 0);

        state.complete_instruction(None);
        assert_eq!(state.pc(), 0);
        assert_eq!(state.delay_slot, None);
    }

    #[test]
    fn exception_outside_delay_slot_uses_current_pc() {
        let mut state = State::new();
        state.pc = 0xbfc0_0040;
        state.write_gpr(1, 0x1234_5678);
        let mut expected_cp0 = Cp0::new();
        let expected_pc = expected_cp0.take_exception(Exception::Syscall, state.pc, false);

        state.take_exception(Exception::Syscall);

        assert_eq!(state.cp0, expected_cp0);
        assert_eq!(state.pc, expected_pc);
        assert_eq!(state.delay_slot, None);
        assert_eq!(state.read_gpr(1), 0x1234_5678);
    }

    #[test]
    fn exception_in_delay_slot_uses_origin_and_cancels_resume() {
        let mut state = State::new();
        let origin_pc = state.pc;
        let resume_pc = 0xbfc0_0040;
        state.write_gpr(31, origin_pc.wrapping_add(8));
        state.complete_instruction(Some(resume_pc));
        let mut expected_cp0 = Cp0::new();
        let expected_pc = expected_cp0.take_exception(Exception::Overflow, origin_pc, true);

        state.take_exception(Exception::Overflow);

        assert_eq!(state.cp0, expected_cp0);
        assert_eq!(state.pc, expected_pc);
        assert_eq!(state.delay_slot, None);
        assert_eq!(state.read_gpr(31), origin_pc.wrapping_add(8));
    }
}
