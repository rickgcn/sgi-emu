//! Dynamic MIPS architecture-level instruction access checks.

use crate::cpu::mips4::cp0::Mips4Cp0Status;
use crate::cpu::mips4::exception::{Mips4CoprocessorNumber, Mips4Exception};
use crate::cpu::mips4::instruction::requirements::{
    Mips4ArchitectureLevel, Mips4DisabledInstructionAction, Mips4InstructionRequirements,
};
use crate::cpu::mips4::mmu::Mips4MmuPrivilegeMode;

/// Result of checking dynamic instruction access requirements.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4InstructionAccess {
    /// The instruction may execute.
    Execute,
    /// The instruction raises an architectural exception.
    Exception(Mips4Exception),
    /// CP1 must record and raise an unimplemented-operation exception.
    FloatingPointUnimplemented,
}

/// Checks whether the current mode enables an instruction architecture level.
pub const fn check_architecture_level(
    status: Mips4Cp0Status,
    requirements: Mips4InstructionRequirements,
) -> Mips4InstructionAccess {
    use Mips4ArchitectureLevel::{Mips1, Mips2, Mips3, Mips4};

    let Some(mode) = Mips4MmuPrivilegeMode::from_status(status) else {
        return reserved();
    };
    let enabled = match (mode, requirements.architecture_level) {
        (_, Mips1 | Mips2) | (Mips4MmuPrivilegeMode::Kernel, _) => true,
        (Mips4MmuPrivilegeMode::Supervisor, Mips3) => status.supervisor_64_bit_addressing(),
        (Mips4MmuPrivilegeMode::Supervisor, Mips4) => true,
        (Mips4MmuPrivilegeMode::User, Mips3) => status.user_64_bit_addressing(),
        (Mips4MmuPrivilegeMode::User, Mips4) => status.xx(),
    };
    if enabled {
        Mips4InstructionAccess::Execute
    } else {
        match requirements.disabled_action {
            Mips4DisabledInstructionAction::ReservedInstruction => reserved(),
            Mips4DisabledInstructionAction::FloatingPointUnimplemented => {
                Mips4InstructionAccess::FloatingPointUnimplemented
            }
        }
    }
}

/// Checks architecture-level, presence, and usability requirements for a coprocessor.
pub const fn check_coprocessor_access(
    status: Mips4Cp0Status,
    present: bool,
    coprocessor: Mips4CoprocessorNumber,
    requirements: Mips4InstructionRequirements,
) -> Mips4InstructionAccess {
    let architecture_access = check_architecture_level(status, requirements);
    if !matches!(architecture_access, Mips4InstructionAccess::Execute) {
        return architecture_access;
    }
    if !status.coprocessor_usable(coprocessor) {
        return coprocessor_unusable(coprocessor);
    }
    if !present {
        return reserved();
    }
    Mips4InstructionAccess::Execute
}

pub(super) const fn coprocessor_unusable(
    coprocessor: Mips4CoprocessorNumber,
) -> Mips4InstructionAccess {
    Mips4InstructionAccess::Exception(Mips4Exception::CoprocessorUnusable { coprocessor })
}

pub(super) const fn reserved() -> Mips4InstructionAccess {
    Mips4InstructionAccess::Exception(Mips4Exception::ReservedInstruction)
}

#[cfg(test)]
mod tests {
    use crate::cpu::mips4::cp1::decode::decode_instruction as decode_cp1;
    use crate::cpu::mips4::instruction::Mips4Instruction;

    use super::super::fpu::check_fpu_access;
    use super::*;

    const STATUS_XX: u32 = 1 << 31;
    const STATUS_CU1: u32 = 1 << 29;
    const STATUS_SX: u32 = 1 << 6;
    const STATUS_UX: u32 = 1 << 5;
    const STATUS_KSU_SHIFT: u8 = 3;
    const STATUS_ERL: u32 = 1 << 2;
    const STATUS_EXL: u32 = 1 << 1;

    const MIPS3: Mips4InstructionRequirements =
        Mips4InstructionRequirements::new(Mips4ArchitectureLevel::Mips3);
    const MIPS4: Mips4InstructionRequirements =
        Mips4InstructionRequirements::new(Mips4ArchitectureLevel::Mips4);

    fn status(bits: u32) -> Mips4Cp0Status {
        Mips4Cp0Status::from_bits(bits)
    }

    fn supervisor(bits: u32) -> Mips4Cp0Status {
        status((1 << STATUS_KSU_SHIFT) | bits)
    }

    fn user(bits: u32) -> Mips4Cp0Status {
        status((2 << STATUS_KSU_SHIFT) | bits)
    }

    fn fpu_access(bits: u32, status: Mips4Cp0Status) -> Mips4InstructionAccess {
        let raw = Mips4Instruction::from_bits(bits);
        check_fpu_access(status, true, raw, decode_cp1(raw).unwrap())
    }

    #[test]
    fn architecture_level_access_matches_mode_matrix() {
        assert_eq!(
            check_architecture_level(status(0), MIPS3),
            Mips4InstructionAccess::Execute
        );
        assert_eq!(
            check_architecture_level(status(0), MIPS4),
            Mips4InstructionAccess::Execute
        );
        assert_eq!(check_architecture_level(supervisor(0), MIPS3), reserved());
        assert_eq!(
            check_architecture_level(supervisor(0), MIPS4),
            Mips4InstructionAccess::Execute
        );
        assert_eq!(
            check_architecture_level(supervisor(STATUS_SX), MIPS3),
            Mips4InstructionAccess::Execute
        );
        assert_eq!(check_architecture_level(user(0), MIPS3), reserved());
        assert_eq!(check_architecture_level(user(0), MIPS4), reserved());
        assert_eq!(
            check_architecture_level(user(STATUS_UX), MIPS3),
            Mips4InstructionAccess::Execute
        );
        assert_eq!(check_architecture_level(user(STATUS_UX), MIPS4), reserved());
        assert_eq!(check_architecture_level(user(STATUS_XX), MIPS3), reserved());
        assert_eq!(
            check_architecture_level(user(STATUS_XX), MIPS4),
            Mips4InstructionAccess::Execute
        );
        assert_eq!(
            check_architecture_level(user(STATUS_EXL), MIPS4),
            Mips4InstructionAccess::Execute
        );
        assert_eq!(
            check_architecture_level(supervisor(STATUS_ERL), MIPS3),
            Mips4InstructionAccess::Execute
        );
    }

    #[test]
    fn table_17_4_access_precedence_is_preserved() {
        let movf = (1_u32 << 16) | 0x01;
        assert_eq!(fpu_access(movf, user(0)), reserved());
        assert_eq!(
            fpu_access(movf, user(STATUS_XX)),
            coprocessor_unusable(Mips4CoprocessorNumber::Cp1)
        );
        assert_eq!(
            fpu_access(movf, user(STATUS_XX | STATUS_CU1)),
            Mips4InstructionAccess::Execute
        );

        let bc1_cc1 = (0x11_u32 << 26) | (0x08 << 21) | (4 << 16);
        assert_eq!(fpu_access(bc1_cc1, user(0)), reserved());
        assert_eq!(fpu_access(bc1_cc1, user(STATUS_CU1)), reserved());
        assert_eq!(
            fpu_access(bc1_cc1, user(STATUS_XX)),
            coprocessor_unusable(Mips4CoprocessorNumber::Cp1)
        );

        let compare_cc1 = (0x11_u32 << 26) | (0x10 << 21) | (4 << 6) | 0x32;
        let movz_s = (0x11_u32 << 26) | (0x10 << 21) | 0x12;
        let recip_s = (0x11_u32 << 26) | (0x10 << 21) | 0x15;
        for bits in [compare_cc1, movz_s, recip_s] {
            assert_eq!(
                fpu_access(bits, user(STATUS_CU1)),
                Mips4InstructionAccess::FloatingPointUnimplemented
            );
        }

        let dmfc1 = (0x11_u32 << 26) | (0x01 << 21);
        assert_eq!(fpu_access(dmfc1, user(STATUS_CU1)), reserved());
        assert_eq!(
            fpu_access(dmfc1, user(STATUS_CU1 | STATUS_UX)),
            Mips4InstructionAccess::Execute
        );

        let cop1x = (0x13_u32 << 26) | 0x20;
        assert_eq!(fpu_access(cop1x, user(STATUS_CU1)), reserved());
        assert_eq!(fpu_access(cop1x, user(0)), reserved());
        assert_eq!(
            fpu_access(cop1x, user(STATUS_XX)),
            coprocessor_unusable(Mips4CoprocessorNumber::Cp1)
        );
        assert_eq!(
            fpu_access(cop1x, user(STATUS_XX | STATUS_CU1)),
            Mips4InstructionAccess::Execute
        );
    }

    #[test]
    fn absent_cp1_never_executes() {
        let add_s = Mips4Instruction::from_bits((0x11_u32 << 26) | (0x10 << 21));
        assert_eq!(
            check_fpu_access(user(0), false, add_s, decode_cp1(add_s).unwrap()),
            coprocessor_unusable(Mips4CoprocessorNumber::Cp1)
        );
        assert_eq!(
            check_fpu_access(user(STATUS_CU1), false, add_s, decode_cp1(add_s).unwrap(),),
            reserved()
        );
    }

    #[test]
    fn cp2_presence_and_usability_are_checked_independently() {
        let requirements = Mips4InstructionRequirements::new(Mips4ArchitectureLevel::Mips1);
        assert_eq!(
            check_coprocessor_access(status(0), false, Mips4CoprocessorNumber::Cp2, requirements,),
            coprocessor_unusable(Mips4CoprocessorNumber::Cp2)
        );
        assert_eq!(
            check_coprocessor_access(
                status(1 << 30),
                false,
                Mips4CoprocessorNumber::Cp2,
                requirements,
            ),
            reserved()
        );
        assert_eq!(
            check_coprocessor_access(
                status(1 << 30),
                true,
                Mips4CoprocessorNumber::Cp2,
                requirements,
            ),
            Mips4InstructionAccess::Execute
        );
    }
}
