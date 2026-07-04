use super::*;

#[test]
fn default_control_uses_nearest_even_and_after_rounding() {
    let control = FloatControl::default();

    assert_eq!(control.rounding_mode, FloatRoundingMode::NearestEven);
    assert_eq!(control.tininess_mode, FloatTininessMode::AfterRounding);
}

#[test]
fn control_can_select_rounding_mode() {
    let control = FloatControl::with_rounding_mode(FloatRoundingMode::TowardZero);

    assert_eq!(control.rounding_mode, FloatRoundingMode::TowardZero);
    assert_eq!(control.tininess_mode, FloatTininessMode::AfterRounding);
}

#[test]
fn flags_support_bits_union_and_contains() {
    let flags = FloatExceptionFlags::INVALID | FloatExceptionFlags::INEXACT;

    assert_eq!(flags.bits(), 0x11);
    assert!(flags.contains(FloatExceptionFlags::INVALID));
    assert!(flags.contains(FloatExceptionFlags::INEXACT));
    assert!(!flags.contains(FloatExceptionFlags::OVERFLOW));
}

#[test]
fn flags_truncate_unknown_bits() {
    let flags = FloatExceptionFlags::from_bits_truncate(0xff);

    assert_eq!(flags.bits(), 0x1f);
}

#[test]
fn empty_flags_have_no_bits() {
    let flags = FloatExceptionFlags::empty();

    assert!(flags.is_empty());
    assert_eq!(flags.bits(), 0);
}
