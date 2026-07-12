use super::*;

#[test]
fn bank_registers_are_eight_byte_spaced() {
    assert_eq!(memory_bank_control(0), Some(0x1400_0208));
    assert_eq!(memory_bank_control(7), Some(0x1400_0240));
    assert_eq!(memory_bank_control(8), None);
    assert_eq!(memory_bank_index(0x1400_0228), Some(4));
    assert_eq!(memory_bank_index(0x1400_022c), None);
}

#[test]
fn crime_11_identity_contains_asic_and_revision() {
    assert_eq!(ID_VALUE >> 4, 0xA);
    assert_eq!(ID_VALUE & 0xF, 1);
}
