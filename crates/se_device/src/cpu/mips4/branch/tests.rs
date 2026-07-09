use super::*;

#[test]
fn beq_and_bne_report_taken_and_not_taken_without_nullifying_normal_delay_slots() {
    assert_eq!(
        Mips4Branch::beq(0x1000, 7, 7, 2),
        Mips4BranchDecision::Taken {
            target: 0x100c,
            nullify_delay_slot: false,
        }
    );
    assert_eq!(
        Mips4Branch::beq(0x1000, 7, 8, 2),
        Mips4BranchDecision::NotTaken {
            nullify_delay_slot: false,
        }
    );
    assert_eq!(
        Mips4Branch::bne(0x1000, 7, 8, 2),
        Mips4BranchDecision::Taken {
            target: 0x100c,
            nullify_delay_slot: false,
        }
    );
}

#[test]
fn branch_likely_nullifies_only_when_not_taken() {
    assert_eq!(
        Mips4Branch::beql(0x1000, 7, 7, 2),
        Mips4BranchDecision::Taken {
            target: 0x100c,
            nullify_delay_slot: false,
        }
    );
    assert_eq!(
        Mips4Branch::beql(0x1000, 7, 8, 2),
        Mips4BranchDecision::NotTaken {
            nullify_delay_slot: true,
        }
    );
    assert_eq!(
        Mips4Branch::bnel(0x1000, 7, 7, 2),
        Mips4BranchDecision::NotTaken {
            nullify_delay_slot: true,
        }
    );
}

#[test]
fn sign_branches_use_doubleword_signed_comparisons() {
    assert!(Mips4Branch::bltz(0, u64::MAX, 1).is_taken());
    assert!(!Mips4Branch::bltz(0, 0, 1).is_taken());
    assert!(Mips4Branch::bgez(0, 0, 1).is_taken());
    assert!(Mips4Branch::blez(0, 0, 1).is_taken());
    assert!(!Mips4Branch::bgtz(0, 0, 1).is_taken());
    assert!(Mips4Branch::bgtz(0, 1, 1).is_taken());
    assert!(Mips4Branch::bgtzl(0, 1, 1).is_taken());
    assert!(Mips4Branch::blezl(0, u64::MAX, 1).is_taken());
}

#[test]
fn branch_target_is_relative_to_delay_slot() {
    assert_eq!(Mips4Branch::beq(0x1000, 1, 1, -1).target(), Some(0x1000));
    assert_eq!(Mips4Branch::beq(0x1000, 1, 1, -2).target(), Some(0x0ffc));
}

#[test]
fn linked_branches_return_link_value_even_when_not_taken() {
    assert_eq!(
        Mips4Branch::bltzal(0x2000, 0, 4),
        Mips4LinkedBranchDecision {
            decision: Mips4BranchDecision::NotTaken {
                nullify_delay_slot: false,
            },
            link_value: 0x2008,
        }
    );
    assert_eq!(
        Mips4Branch::bltzall(0x2000, 0, 4),
        Mips4LinkedBranchDecision {
            decision: Mips4BranchDecision::NotTaken {
                nullify_delay_slot: true,
            },
            link_value: 0x2008,
        }
    );
    assert_eq!(Mips4Branch::bgezal(0x2000, 0, 4).link_value, 0x2008);
    assert_eq!(Mips4Branch::bgezall(0x2000, u64::MAX, 4).link_value, 0x2008);
}

#[test]
fn jump_targets_use_delay_slot_high_bits() {
    assert_eq!(
        Mips4Branch::j(0xffff_ffff_0fff_fffc, 0x0000_0001),
        Mips4BranchDecision::Taken {
            target: 0xffff_ffff_1000_0004,
            nullify_delay_slot: false,
        }
    );
    assert_eq!(
        Mips4Branch::jal(0xffff_ffff_0fff_fffc, 1).link_value,
        0xffff_ffff_1000_0004
    );
}

#[test]
fn register_jumps_reject_unaligned_targets() {
    assert_eq!(
        Mips4Branch::jr(0xffff_ffff_8000_0000),
        Ok(Mips4BranchDecision::Taken {
            target: 0xffff_ffff_8000_0000,
            nullify_delay_slot: false,
        })
    );
    assert_eq!(
        Mips4Branch::jr(0xffff_ffff_8000_0002),
        Err(Mips4Exception::AddressErrorLoad)
    );
    assert_eq!(
        Mips4Branch::jalr(0x1000, 0xffff_ffff_8000_0002),
        Err(Mips4Exception::AddressErrorLoad)
    );
}

#[test]
fn bc1_branches_take_on_matching_condition_without_nullifying_normal_delay_slots() {
    assert_eq!(
        Mips4Branch::bc1f(0x1000, false, 2),
        Mips4BranchDecision::Taken {
            target: 0x100c,
            nullify_delay_slot: false,
        }
    );
    assert_eq!(
        Mips4Branch::bc1f(0x1000, true, 2),
        Mips4BranchDecision::NotTaken {
            nullify_delay_slot: false,
        }
    );
    assert_eq!(
        Mips4Branch::bc1t(0x1000, true, 2),
        Mips4BranchDecision::Taken {
            target: 0x100c,
            nullify_delay_slot: false,
        }
    );
    assert_eq!(
        Mips4Branch::bc1t(0x1000, false, 2),
        Mips4BranchDecision::NotTaken {
            nullify_delay_slot: false,
        }
    );
}

#[test]
fn bc1_likely_branches_nullify_only_when_not_taken() {
    assert_eq!(
        Mips4Branch::bc1fl(0x1000, false, 2),
        Mips4BranchDecision::Taken {
            target: 0x100c,
            nullify_delay_slot: false,
        }
    );
    assert_eq!(
        Mips4Branch::bc1fl(0x1000, true, 2),
        Mips4BranchDecision::NotTaken {
            nullify_delay_slot: true,
        }
    );
    assert_eq!(
        Mips4Branch::bc1tl(0x1000, true, 2),
        Mips4BranchDecision::Taken {
            target: 0x100c,
            nullify_delay_slot: false,
        }
    );
    assert_eq!(
        Mips4Branch::bc1tl(0x1000, false, 2),
        Mips4BranchDecision::NotTaken {
            nullify_delay_slot: true,
        }
    );
}
