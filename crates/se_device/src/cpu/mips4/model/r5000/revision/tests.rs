use super::*;

#[test]
fn implementation_id_matches_r5000() {
    assert_eq!(R5000_IMPLEMENTATION_ID, 0x23);
}

#[test]
fn revision_preserves_raw_bits_and_extracts_nibbles() {
    let revision = R5000Revision::from_bits(0x2a);

    assert_eq!(revision.bits(), 0x2a);
    assert_eq!(revision.major(), 0x02);
    assert_eq!(revision.minor(), 0x0a);
}

#[test]
fn identity_registers_use_r5000_implementation_field() {
    let revision = R5000Revision::from_bits(0x41);

    assert_eq!(r5000_processor_id(revision), 0x0000_2341);
    assert_eq!(r5000_fcr0(revision), 0x0000_2341);
}
