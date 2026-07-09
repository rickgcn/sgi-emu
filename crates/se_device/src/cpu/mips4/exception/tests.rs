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
