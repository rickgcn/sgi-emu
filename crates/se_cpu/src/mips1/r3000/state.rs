use super::cp0::{Cp0, Exception};

const RESET_PC: u32 = 0xbfc0_0000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DelaySlot {
    origin_pc: u32,
    resume_pc: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingGprWrite {
    index: usize,
    value: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingCp0Write {
    index: usize,
    value: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum InstructionEffect {
    DelayedGprWrite { index: usize, value: u32 },
    DelayedCp0Write { index: usize, value: u32 },
    RestoreStatus { value: u32 },
}

pub(super) struct State {
    gpr: [u32; 32],
    hi: u32,
    lo: u32,
    pc: u32,
    delay_slot: Option<DelaySlot>,
    cp0: Cp0,
    pending_gpr_write: Option<PendingGprWrite>,
    pending_cp0_write: Option<PendingCp0Write>,
}

impl State {
    pub(super) fn new() -> Self {
        Self {
            gpr: [0; 32],
            hi: 0,
            lo: 0,
            pc: RESET_PC,
            delay_slot: None,
            cp0: Cp0::new(),
            pending_gpr_write: None,
            pending_cp0_write: None,
        }
    }

    pub(super) fn reset(&mut self) {
        let interrupted_pc = self.pc;
        self.cp0.reset(interrupted_pc);
        self.gpr[0] = 0;
        self.pc = RESET_PC;
        self.delay_slot = None;
        self.pending_gpr_write = None;
        self.pending_cp0_write = None;
    }

    pub(super) fn pc(&self) -> u32 {
        self.pc
    }

    pub(super) fn read_gpr(&self, index: usize) -> u32 {
        if index == 0 { 0 } else { self.gpr[index] }
    }

    pub(super) fn write_gpr(&mut self, index: usize, value: u32) {
        self.commit_pending_gpr_write();
        self.write_gpr_direct(index, value);
    }

    fn write_gpr_direct(&mut self, index: usize, value: u32) {
        if index != 0 {
            self.gpr[index] = value;
        }
    }

    pub(super) fn read_hi(&self) -> u32 {
        self.hi
    }

    pub(super) fn write_hi(&mut self, value: u32) {
        self.hi = value;
    }

    pub(super) fn read_lo(&self) -> u32 {
        self.lo
    }

    pub(super) fn write_lo(&mut self, value: u32) {
        self.lo = value;
    }

    pub(super) fn read_cp0(&self, index: usize) -> u32 {
        self.cp0.read_register(index)
    }

    pub(super) fn cp0_status(&self) -> u32 {
        self.cp0.status()
    }

    pub(super) fn cp0_usable(&self) -> bool {
        self.cp0.is_usable()
    }

    pub(super) fn complete_instruction(
        &mut self,
        delayed_resume_pc: Option<u32>,
        effect: Option<InstructionEffect>,
    ) {
        self.cp0.commit_pending_functional();
        self.commit_pending_gpr_write();
        self.commit_pending_cp0_write();

        match effect {
            Some(InstructionEffect::DelayedGprWrite { index, value }) => {
                self.pending_gpr_write = Some(PendingGprWrite { index, value });
            }
            Some(InstructionEffect::DelayedCp0Write { index, value }) => {
                self.pending_cp0_write = Some(PendingCp0Write { index, value });
            }
            Some(InstructionEffect::RestoreStatus { value }) => {
                self.cp0.restore_status(value);
            }
            None => {}
        }

        let origin_pc = self.pc;

        self.pc = match self.delay_slot.take() {
            Some(delay_slot) => delay_slot.resume_pc,
            None => self.pc.wrapping_add(4),
        };

        self.delay_slot = delayed_resume_pc.map(|resume_pc| DelaySlot {
            origin_pc,
            resume_pc,
        });
        self.cp0.advance_random();
    }

    pub(super) fn take_exception(&mut self, exception: Exception) {
        self.cp0.commit_pending_functional();
        self.commit_pending_gpr_write();
        self.commit_pending_cp0_write();

        let (epc, in_delay_slot) = match self.delay_slot.take() {
            Some(delay_slot) => (delay_slot.origin_pc, true),
            None => (self.pc, false),
        };

        self.pc = self.cp0.take_exception(exception, epc, in_delay_slot);
        self.cp0.advance_random();
    }

    fn commit_pending_gpr_write(&mut self) {
        if let Some(write) = self.pending_gpr_write.take() {
            self.write_gpr_direct(write.index, write.value);
        }
    }

    fn commit_pending_cp0_write(&mut self) {
        if let Some(write) = self.pending_cp0_write.take() {
            self.cp0.write_register(write.index, write.value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Cp0, DelaySlot, Exception, InstructionEffect, PendingCp0Write, PendingGprWrite, RESET_PC,
        State,
    };

    #[test]
    fn new_initializes_deterministic_state() {
        let state = State::new();

        assert_eq!(state.gpr, [0; 32]);
        assert_eq!(state.hi, 0);
        assert_eq!(state.lo, 0);
        assert_eq!(state.pc, RESET_PC);
        assert_eq!(state.delay_slot, None);
        assert_eq!(state.cp0, Cp0::new());
        assert_eq!(state.pending_gpr_write, None);
        assert_eq!(state.pending_cp0_write, None);
    }

    #[test]
    fn reset_restores_defined_state_only() {
        let mut state = State::new();
        for (index, register) in state.gpr.iter_mut().enumerate() {
            *register = index as u32 + 1;
        }
        state.write_hi(0x1234_5678);
        state.write_lo(0x89ab_cdef);
        state.pc = 0;
        state.delay_slot = Some(DelaySlot {
            origin_pc: 0xffff_fff8,
            resume_pc: 0x1234_5678,
        });
        state.pending_gpr_write = Some(PendingGprWrite {
            index: 1,
            value: 0xaaaa_aaaa,
        });
        state.pending_cp0_write = Some(PendingCp0Write {
            index: 14,
            value: 0xbbbb_bbbb,
        });
        let preserved_gpr = state.gpr;
        let preserved_hi = state.read_hi();
        let preserved_lo = state.read_lo();
        let mut expected_cp0 = Cp0::new();
        expected_cp0.reset(state.pc);

        state.reset();

        assert_eq!(state.gpr[0], 0);
        assert_eq!(state.gpr[1..], preserved_gpr[1..]);
        assert_eq!(state.read_hi(), preserved_hi);
        assert_eq!(state.read_lo(), preserved_lo);
        assert_eq!(state.pc, RESET_PC);
        assert_eq!(state.delay_slot, None);
        assert_eq!(state.cp0, expected_cp0);
        assert_eq!(state.pending_gpr_write, None);
        assert_eq!(state.pending_cp0_write, None);
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
    fn hi_and_lo_accessors_are_independent() {
        let mut state = State::new();

        state.write_hi(0x1234_5678);
        assert_eq!(state.read_hi(), 0x1234_5678);
        assert_eq!(state.read_lo(), 0);

        state.write_lo(0x89ab_cdef);
        assert_eq!(state.read_hi(), 0x1234_5678);
        assert_eq!(state.read_lo(), 0x89ab_cdef);
    }

    #[test]
    fn sequential_completion_advances_with_wrapping_arithmetic() {
        let mut state = State::new();

        assert_eq!(state.pc(), RESET_PC);
        state.complete_instruction(None, None);
        assert_eq!(state.pc(), RESET_PC + 4);

        state.pc = 0xffff_fffc;
        state.complete_instruction(None, None);
        assert_eq!(state.pc(), 0);
    }

    #[test]
    fn control_flow_completion_enters_and_leaves_delay_slot() {
        let mut state = State::new();
        let resume_pc = 0xbfc0_0040;

        state.complete_instruction(Some(resume_pc), None);

        assert_eq!(state.pc(), RESET_PC + 4);
        assert_eq!(
            state.delay_slot,
            Some(DelaySlot {
                origin_pc: RESET_PC,
                resume_pc,
            })
        );

        state.complete_instruction(None, None);

        assert_eq!(state.pc(), resume_pc);
        assert_eq!(state.delay_slot, None);
    }

    #[test]
    fn not_taken_branch_still_records_delay_slot_origin() {
        let mut state = State::new();
        let fallthrough = RESET_PC + 8;

        state.complete_instruction(Some(fallthrough), None);

        assert_eq!(
            state.delay_slot,
            Some(DelaySlot {
                origin_pc: RESET_PC,
                resume_pc: fallthrough,
            })
        );

        state.complete_instruction(None, None);

        assert_eq!(state.pc(), fallthrough);
        assert_eq!(state.delay_slot, None);
    }

    #[test]
    fn delay_slot_resume_address_can_wrap_to_zero() {
        let mut state = State::new();
        state.pc = 0xffff_fffc;

        state.complete_instruction(Some(0), None);
        assert_eq!(state.pc(), 0);

        state.complete_instruction(None, None);
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
        expected_cp0.advance_random();

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
        state.complete_instruction(Some(resume_pc), None);
        let mut expected_cp0 = Cp0::new();
        let expected_pc = expected_cp0.take_exception(Exception::Overflow, origin_pc, true);
        expected_cp0.advance_random();
        expected_cp0.advance_random();

        state.take_exception(Exception::Overflow);

        assert_eq!(state.cp0, expected_cp0);
        assert_eq!(state.pc, expected_pc);
        assert_eq!(state.delay_slot, None);
        assert_eq!(state.read_gpr(31), origin_pc.wrapping_add(8));
    }

    #[test]
    fn delayed_gpr_write_becomes_visible_after_one_instruction() {
        let mut state = State::new();
        state.write_gpr(1, 0x1111_1111);

        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedGprWrite {
                index: 1,
                value: 0x2222_2222,
            }),
        );

        assert_eq!(state.read_gpr(1), 0x1111_1111);
        let dependent_value = state.read_gpr(1);
        state.complete_instruction(None, None);

        assert_eq!(dependent_value, 0x1111_1111);
        assert_eq!(state.read_gpr(1), 0x2222_2222);

        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedGprWrite {
                index: 0,
                value: u32::MAX,
            }),
        );
        state.complete_instruction(None, None);
        assert_eq!(state.read_gpr(0), 0);
    }

    #[test]
    fn direct_gpr_write_overrides_a_pending_transfer() {
        let mut state = State::new();
        state.write_gpr(1, 0x1111_1111);
        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedGprWrite {
                index: 1,
                value: 0x2222_2222,
            }),
        );

        let source = state.read_gpr(1);
        state.write_gpr(1, source.wrapping_add(1));
        state.complete_instruction(None, None);

        assert_eq!(source, 0x1111_1111);
        assert_eq!(state.read_gpr(1), 0x1111_1112);
        assert_eq!(state.pending_gpr_write, None);
    }

    #[test]
    fn consecutive_transfers_commit_in_order() {
        let mut state = State::new();
        state.write_gpr(1, 0x1111_1111);

        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedGprWrite {
                index: 1,
                value: 0x2222_2222,
            }),
        );
        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedGprWrite {
                index: 1,
                value: 0x3333_3333,
            }),
        );

        assert_eq!(state.read_gpr(1), 0x2222_2222);
        state.complete_instruction(None, None);
        assert_eq!(state.read_gpr(1), 0x3333_3333);

        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp0Write {
                index: 14,
                value: 0x4444_4444,
            }),
        );
        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp0Write {
                index: 14,
                value: 0x5555_5555,
            }),
        );

        assert_eq!(state.read_cp0(14), 0x4444_4444);
        state.complete_instruction(None, None);
        assert_eq!(state.read_cp0(14), 0x5555_5555);
    }

    #[test]
    fn exception_commits_old_transfers_before_hardware_state() {
        let mut state = State::new();
        let exception_pc = state.pc().wrapping_add(8);

        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedGprWrite {
                index: 1,
                value: 0x1234_5678,
            }),
        );
        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp0Write {
                index: 14,
                value: 0xdead_beef,
            }),
        );
        assert_eq!(state.pc(), exception_pc);

        state.take_exception(Exception::Syscall);

        assert_eq!(state.read_gpr(1), 0x1234_5678);
        assert_eq!(state.read_cp0(14), exception_pc);
        assert_eq!(state.pending_gpr_write, None);
        assert_eq!(state.pending_cp0_write, None);
    }

    #[test]
    fn transfer_delay_coexists_with_a_branch_delay_slot() {
        let mut state = State::new();
        let resume_pc = 0xbfc0_0040;
        state.write_gpr(1, 0x1111_1111);

        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedGprWrite {
                index: 1,
                value: 0x2222_2222,
            }),
        );
        let branch_source = state.read_gpr(1);
        state.complete_instruction(Some(resume_pc), None);

        assert_eq!(branch_source, 0x1111_1111);
        assert_eq!(state.read_gpr(1), 0x2222_2222);
        assert!(state.delay_slot.is_some());

        state.complete_instruction(None, None);

        assert_eq!(state.pc(), resume_pc);
        assert_eq!(state.delay_slot, None);
    }

    #[test]
    fn status_functional_state_uses_the_full_hazard_window() {
        const STATUS_BEV: u32 = 1 << 22;
        const STATUS_KUC: u32 = 1 << 1;
        const STATUS_CU0: u32 = 1 << 28;

        let mut state = State::new();
        let user_with_cu0 = STATUS_BEV | STATUS_KUC | STATUS_CU0;

        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp0Write {
                index: 12,
                value: user_with_cu0,
            }),
        );
        assert_eq!(state.read_cp0(12), STATUS_BEV);
        assert!(state.cp0_usable());

        state.complete_instruction(None, None);
        assert_eq!(state.read_cp0(12), user_with_cu0);
        assert!(!state.cp0_usable());

        state.complete_instruction(None, None);
        assert!(state.cp0_usable());
    }

    #[test]
    fn rfe_restore_overrides_pending_status_stack_only() {
        const STATUS_BEV: u32 = 1 << 22;
        const STATUS_CU0: u32 = 1 << 28;

        let mut state = State::new();
        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp0Write {
                index: 12,
                value: STATUS_BEV | STATUS_CU0 | 0x0c,
            }),
        );

        state.complete_instruction(
            None,
            Some(InstructionEffect::RestoreStatus { value: STATUS_BEV }),
        );

        assert_eq!(state.read_cp0(12), STATUS_BEV | STATUS_CU0);
    }

    #[test]
    fn reset_clears_functional_and_transfer_pending_state() {
        const STATUS_BEV: u32 = 1 << 22;
        const STATUS_KUC: u32 = 1 << 1;
        const STATUS_CU0: u32 = 1 << 28;

        let mut state = State::new();
        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp0Write {
                index: 12,
                value: STATUS_BEV | STATUS_KUC | STATUS_CU0,
            }),
        );
        state.complete_instruction(None, None);
        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedGprWrite {
                index: 1,
                value: 0x1234_5678,
            }),
        );

        state.reset();

        assert_eq!(state.pending_gpr_write, None);
        assert_eq!(state.pending_cp0_write, None);
        assert_eq!(state.read_cp0(12) & STATUS_KUC, 0);
        assert!(state.cp0_usable());
    }

    #[test]
    fn random_advances_for_normal_and_exception_completion() {
        let mut state = State::new();

        assert_eq!(state.read_cp0(1), 63 << 8);
        state.complete_instruction(None, None);
        assert_eq!(state.read_cp0(1), 62 << 8);
        state.take_exception(Exception::Syscall);
        assert_eq!(state.read_cp0(1), 61 << 8);
    }
}
