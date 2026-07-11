use super::*;

#[test]
fn coprocessor_numbers_accept_only_architectural_range() {
    assert_eq!(
        Mips4CoprocessorNumber::from_u8(0),
        Some(Mips4CoprocessorNumber::Cp0)
    );
    assert_eq!(
        Mips4CoprocessorNumber::from_u8(1),
        Some(Mips4CoprocessorNumber::Cp1)
    );
    assert_eq!(
        Mips4CoprocessorNumber::from_u8(2),
        Some(Mips4CoprocessorNumber::Cp2)
    );
    assert_eq!(
        Mips4CoprocessorNumber::from_u8(3),
        Some(Mips4CoprocessorNumber::Cp3)
    );
    assert_eq!(Mips4CoprocessorNumber::from_u8(4), None);
}

#[test]
fn coprocessor_numbers_return_raw_values() {
    assert_eq!(Mips4CoprocessorNumber::Cp0.number(), 0);
    assert_eq!(Mips4CoprocessorNumber::Cp1.number(), 1);
    assert_eq!(Mips4CoprocessorNumber::Cp2.number(), 2);
    assert_eq!(Mips4CoprocessorNumber::Cp3.number(), 3);
}

#[test]
fn exceptions_return_mips4_cause_codes() {
    let cases = [
        (Mips4Exception::Interrupt, 0),
        (Mips4Exception::TlbModification, 1),
        (Mips4Exception::TlbLoad, 2),
        (Mips4Exception::TlbStore, 3),
        (Mips4Exception::AddressErrorLoad, 4),
        (Mips4Exception::AddressErrorStore, 5),
        (Mips4Exception::InstructionBusError, 6),
        (Mips4Exception::DataBusError, 7),
        (Mips4Exception::Syscall, 8),
        (Mips4Exception::Breakpoint, 9),
        (Mips4Exception::ReservedInstruction, 10),
        (
            Mips4Exception::CoprocessorUnusable {
                coprocessor: Mips4CoprocessorNumber::Cp1,
            },
            11,
        ),
        (Mips4Exception::ArithmeticOverflow, 12),
        (Mips4Exception::Trap, 13),
        (Mips4Exception::FloatingPoint, 15),
    ];

    for (exception, code) in cases {
        assert_eq!(exception.cause_code(), code);
    }
}

#[test]
fn coprocessor_unusable_preserves_coprocessor_number() {
    let exception = Mips4Exception::CoprocessorUnusable {
        coprocessor: Mips4CoprocessorNumber::Cp3,
    };

    match exception {
        Mips4Exception::CoprocessorUnusable { coprocessor } => {
            assert_eq!(coprocessor.number(), 3);
        }
        _ => panic!("expected coprocessor unusable exception"),
    }
}

#[test]
fn coprocessor_access_gate_returns_unusable_exception_when_disabled() {
    assert_eq!(
        check_coprocessor_access(Mips4CoprocessorNumber::Cp1, true),
        Ok(())
    );
    assert_eq!(
        check_coprocessor_access(Mips4CoprocessorNumber::Cp2, false),
        Err(Mips4Exception::CoprocessorUnusable {
            coprocessor: Mips4CoprocessorNumber::Cp2
        })
    );
}

#[test]
fn restart_resumes_at_instruction_pc_when_not_in_delay_slot() {
    let restart = Mips4ExceptionRestart::new(0x1000, None);
    assert!(!restart.in_branch_delay_slot);
    assert_eq!(restart.restart_pc, 0x1000);
}

#[test]
fn restart_resumes_at_branch_pc_when_in_delay_slot() {
    let restart = Mips4ExceptionRestart::new(0x1008, Some(0x1000));
    assert!(restart.in_branch_delay_slot);
    assert_eq!(restart.restart_pc, 0x1000);
}

#[test]
fn exception_image_records_reason_restart_and_bad_address() {
    let restart = Mips4ExceptionRestart::new(0x2000, None);
    let image = Mips4ExceptionImage::new(Mips4Exception::TlbLoad, restart, Some(0xdead_beef));

    assert_eq!(image.reason, Mips4Exception::TlbLoad);
    assert_eq!(image.reason.cause_code(), 2);
    assert_eq!(image.restart, restart);
    assert_eq!(image.bad_virtual_address, Some(0xdead_beef));
}

#[test]
fn exception_image_supports_exceptions_without_a_bad_address() {
    let image = Mips4ExceptionImage::new(
        Mips4Exception::ArithmeticOverflow,
        Mips4ExceptionRestart::new(0x3000, None),
        None,
    );
    assert_eq!(image.reason.cause_code(), 12);
    assert_eq!(image.bad_virtual_address, None);
}

#[test]
fn error_exception_images_distinguish_reset_nmi_and_cache_errors() {
    let restart = Mips4ExceptionRestart::new(0xffff_ffff_8000_1004, Some(0xffff_ffff_8000_1000));

    assert_eq!(
        Mips4ErrorExceptionImage::new(Mips4ErrorException::SoftReset, restart),
        Mips4ErrorExceptionImage {
            reason: Mips4ErrorException::SoftReset,
            restart,
            cache_error: None,
        }
    );
    assert_eq!(
        Mips4ErrorExceptionImage::new(Mips4ErrorException::NonMaskableInterrupt, restart),
        Mips4ErrorExceptionImage {
            reason: Mips4ErrorException::NonMaskableInterrupt,
            restart,
            cache_error: None,
        }
    );
    assert_eq!(
        Mips4ErrorExceptionImage::cache_error(restart, 0xa900_1231),
        Mips4ErrorExceptionImage {
            reason: Mips4ErrorException::CacheError,
            restart,
            cache_error: Some(0xa900_1231),
        }
    );
}

#[test]
fn system_exception_kind_maps_to_cause_codes() {
    assert_eq!(
        Mips4SystemExceptionKind::SystemCall.exception(),
        Mips4Exception::Syscall
    );
    assert_eq!(
        Mips4SystemExceptionKind::SystemCall
            .exception()
            .cause_code(),
        8
    );
    assert_eq!(
        Mips4SystemExceptionKind::Breakpoint.exception(),
        Mips4Exception::Breakpoint
    );
    assert_eq!(
        Mips4SystemExceptionKind::Breakpoint
            .exception()
            .cause_code(),
        9
    );
}

#[test]
fn trap_decision_reports_should_trap() {
    assert!(Mips4TrapDecision::Trap.should_trap());
    assert!(!Mips4TrapDecision::Continue.should_trap());
}

#[test]
fn signed_trap_conditions_use_signed_comparisons() {
    assert!(tge(0, -1_i64 as u64).should_trap());
    assert!(!tge(-1_i64 as u64, 0).should_trap());
    assert!(tlt(-1_i64 as u64, 0).should_trap());
    assert!(!tlt(0, -1_i64 as u64).should_trap());
}

#[test]
fn unsigned_trap_conditions_use_unsigned_comparisons() {
    assert!(tgeu(u64::MAX, 0).should_trap());
    assert!(!tgeu(0, u64::MAX).should_trap());
    assert!(tltu(0, u64::MAX).should_trap());
    assert!(!tltu(u64::MAX, 0).should_trap());
}

#[test]
fn equality_trap_conditions_match_exact_values() {
    assert!(teq(7, 7).should_trap());
    assert!(!teq(7, 8).should_trap());
    assert!(tne(7, 8).should_trap());
    assert!(!tne(7, 7).should_trap());
}

#[test]
fn trap_boundaries_distinguish_signed_and_unsigned() {
    assert!(tge(0, i64::MIN as u64).should_trap());
    assert!(!tgeu(0, i64::MIN as u64).should_trap());
    assert!(tlt(i64::MIN as u64, 0).should_trap());
    assert!(!tltu(i64::MIN as u64, 0).should_trap());
}

#[test]
fn immediate_trap_conditions_sign_extend_immediate() {
    assert!(tgei(0, -1).should_trap());
    assert!(!tgei(-1_i64 as u64, 0).should_trap());
    assert!(tgeiu(u64::MAX, -1).should_trap());
    assert!(!tgeiu(0, -1).should_trap());
    assert!(tlti(-1_i64 as u64, 0).should_trap());
    assert!(!tlti(0, -1).should_trap());
    assert!(tltiu(0, -1).should_trap());
    assert!(!tltiu(u64::MAX, -1).should_trap());
    assert!(teqi(-1_i64 as u64, -1).should_trap());
    assert!(!teqi(0, -1).should_trap());
    assert!(tnei(0, -1).should_trap());
    assert!(!tnei(-1_i64 as u64, -1).should_trap());
}
