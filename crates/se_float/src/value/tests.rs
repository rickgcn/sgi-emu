use super::*;

#[test]
fn float32_bits_preserve_fields() {
    let value = Float32Bits::new(0xc020_0001);

    assert_eq!(value.bits(), 0xc020_0001);
    assert!(value.sign_bit());
    assert_eq!(value.exponent_bits(), 0x80);
    assert_eq!(value.fraction_bits(), 0x0020_0001);
}

#[test]
fn float64_bits_preserve_fields() {
    let value = Float64Bits::new(0xc004_0000_0000_0001);

    assert_eq!(value.bits(), 0xc004_0000_0000_0001);
    assert!(value.sign_bit());
    assert_eq!(value.exponent_bits(), 0x400);
    assert_eq!(value.fraction_bits(), 0x0004_0000_0000_0001);
}

#[test]
fn abs_and_neg_preserve_payload_bits() {
    let value = Float32Bits::new(0xffc0_1234);

    assert_eq!(value.abs().bits(), 0x7fc0_1234);
    assert_eq!(value.neg().bits(), 0x7fc0_1234);
}

#[test]
fn classifies_float32_values() {
    assert_eq!(
        Float32Bits::new(0x0000_0000).classify(FloatNanMode::QuietBitSet),
        FloatClass::PositiveZero
    );
    assert_eq!(
        Float32Bits::new(0x8000_0000).classify(FloatNanMode::QuietBitSet),
        FloatClass::NegativeZero
    );
    assert_eq!(
        Float32Bits::new(0x0000_0001).classify(FloatNanMode::QuietBitSet),
        FloatClass::PositiveSubnormal
    );
    assert_eq!(
        Float32Bits::new(0x3f80_0000).classify(FloatNanMode::QuietBitSet),
        FloatClass::PositiveNormal
    );
    assert_eq!(
        Float32Bits::new(0x7f80_0000).classify(FloatNanMode::QuietBitSet),
        FloatClass::PositiveInfinity
    );
}

#[test]
fn classifies_float64_values() {
    assert_eq!(
        Float64Bits::new(0x0000_0000_0000_0000).classify(FloatNanMode::QuietBitSet),
        FloatClass::PositiveZero
    );
    assert_eq!(
        Float64Bits::new(0x8000_0000_0000_0000).classify(FloatNanMode::QuietBitSet),
        FloatClass::NegativeZero
    );
    assert_eq!(
        Float64Bits::new(0x0000_0000_0000_0001).classify(FloatNanMode::QuietBitSet),
        FloatClass::PositiveSubnormal
    );
    assert_eq!(
        Float64Bits::new(0x3ff0_0000_0000_0000).classify(FloatNanMode::QuietBitSet),
        FloatClass::PositiveNormal
    );
    assert_eq!(
        Float64Bits::new(0x7ff0_0000_0000_0000).classify(FloatNanMode::QuietBitSet),
        FloatClass::PositiveInfinity
    );
}

#[test]
fn nan_classification_respects_quiet_bit_set_mode() {
    assert_eq!(
        Float32Bits::new(0x7fc0_0001).classify(FloatNanMode::QuietBitSet),
        FloatClass::QuietNan
    );
    assert_eq!(
        Float32Bits::new(0x7f80_0001).classify(FloatNanMode::QuietBitSet),
        FloatClass::SignalingNan
    );
}

#[test]
fn nan_classification_respects_quiet_bit_clear_mode() {
    assert_eq!(
        Float64Bits::new(0x7ff0_0000_0000_0001).classify(FloatNanMode::QuietBitClear),
        FloatClass::QuietNan
    );
    assert_eq!(
        Float64Bits::new(0x7ff8_0000_0000_0001).classify(FloatNanMode::QuietBitClear),
        FloatClass::SignalingNan
    );
}
