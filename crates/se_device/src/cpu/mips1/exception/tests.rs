use super::*;

#[test]
fn coprocessor_numbers_accept_only_architectural_range() {
    assert_eq!(
        Mips1CoprocessorNumber::from_u8(0),
        Some(Mips1CoprocessorNumber::Cp0)
    );
    assert_eq!(
        Mips1CoprocessorNumber::from_u8(1),
        Some(Mips1CoprocessorNumber::Cp1)
    );
    assert_eq!(
        Mips1CoprocessorNumber::from_u8(2),
        Some(Mips1CoprocessorNumber::Cp2)
    );
    assert_eq!(
        Mips1CoprocessorNumber::from_u8(3),
        Some(Mips1CoprocessorNumber::Cp3)
    );
    assert_eq!(Mips1CoprocessorNumber::from_u8(4), None);
}

#[test]
fn coprocessor_numbers_return_raw_values() {
    assert_eq!(Mips1CoprocessorNumber::Cp0.number(), 0);
    assert_eq!(Mips1CoprocessorNumber::Cp1.number(), 1);
    assert_eq!(Mips1CoprocessorNumber::Cp2.number(), 2);
    assert_eq!(Mips1CoprocessorNumber::Cp3.number(), 3);
}

#[test]
fn exceptions_return_r30xx_cause_codes() {
    let cases = [
        (Mips1Exception::Interrupt, 0),
        (Mips1Exception::TlbModification, 1),
        (Mips1Exception::TlbLoad, 2),
        (Mips1Exception::TlbStore, 3),
        (Mips1Exception::AddressErrorLoad, 4),
        (Mips1Exception::AddressErrorStore, 5),
        (Mips1Exception::InstructionBusError, 6),
        (Mips1Exception::DataBusError, 7),
        (Mips1Exception::Syscall, 8),
        (Mips1Exception::Breakpoint, 9),
        (Mips1Exception::ReservedInstruction, 10),
        (
            Mips1Exception::CoprocessorUnusable {
                coprocessor: Mips1CoprocessorNumber::Cp1,
            },
            11,
        ),
        (Mips1Exception::ArithmeticOverflow, 12),
    ];

    for (exception, code) in cases {
        assert_eq!(exception.cause_code(), code);
    }
}

#[test]
fn coprocessor_unusable_preserves_coprocessor_number() {
    let exception = Mips1Exception::CoprocessorUnusable {
        coprocessor: Mips1CoprocessorNumber::Cp3,
    };

    match exception {
        Mips1Exception::CoprocessorUnusable { coprocessor } => {
            assert_eq!(coprocessor.number(), 3);
        }
        _ => panic!("expected coprocessor unusable exception"),
    }
}
