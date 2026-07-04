use super::*;

#[test]
fn beq_and_bne_report_taken_and_not_taken() {
    assert_eq!(
        Mips1Branch::beq(0x1000, 7, 7, 2),
        Mips1BranchDecision::Taken { target: 0x100c }
    );
    assert_eq!(
        Mips1Branch::beq(0x1000, 7, 8, 2),
        Mips1BranchDecision::NotTaken
    );
    assert_eq!(
        Mips1Branch::bne(0x1000, 7, 8, 2),
        Mips1BranchDecision::Taken { target: 0x100c }
    );
}

#[test]
fn sign_branches_match_negative_zero_and_positive_values() {
    assert!(Mips1Branch::bltz(0, 0xffff_ffff, 1).is_taken());
    assert!(!Mips1Branch::bltz(0, 0, 1).is_taken());
    assert!(Mips1Branch::bgez(0, 0, 1).is_taken());
    assert!(Mips1Branch::blez(0, 0, 1).is_taken());
    assert!(!Mips1Branch::bgtz(0, 0, 1).is_taken());
    assert!(Mips1Branch::bgtz(0, 1, 1).is_taken());
}

#[test]
fn branch_target_is_relative_to_delay_slot() {
    assert_eq!(Mips1Branch::beq(0x1000, 1, 1, -1).target(), Some(0x1000));
    assert_eq!(Mips1Branch::beq(0x1000, 1, 1, -2).target(), Some(0x0ffc));
}

#[test]
fn link_branches_return_link_value_even_when_not_taken() {
    assert_eq!(
        Mips1Branch::bltzal(0x2000, 0, 4),
        Mips1LinkedBranchDecision {
            decision: Mips1BranchDecision::NotTaken,
            link_value: 0x2008,
        }
    );
    assert_eq!(Mips1Branch::bgezal(0x2000, 0, 4).link_value, 0x2008);
}

#[test]
fn jump_targets_use_delay_slot_high_bits() {
    assert_eq!(
        Mips1Branch::j(0x0fff_fffc, 0x0000_0001),
        Mips1BranchDecision::Taken {
            target: 0x1000_0004,
        }
    );
    assert_eq!(Mips1Branch::jal(0x0fff_fffc, 1).link_value, 0x1000_0004);
}

#[test]
fn register_jumps_reject_unaligned_targets() {
    assert_eq!(
        Mips1Branch::jr(0x8000_0000),
        Ok(Mips1BranchDecision::Taken {
            target: 0x8000_0000,
        })
    );
    assert_eq!(
        Mips1Branch::jr(0x8000_0002),
        Err(Mips1Exception::AddressErrorLoad)
    );
    assert_eq!(
        Mips1Branch::jalr(0x1000, 0x8000_0002),
        Err(Mips1Exception::AddressErrorLoad)
    );
}
