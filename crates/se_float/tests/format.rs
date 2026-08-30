use se_float::format::{Float32, Float64};

#[test]
fn binary32_bits_round_trip() {
    for bits in [0, 1, 0x8000_0000, 0x7f80_0000, 0xffff_ffff] {
        assert_eq!(Float32::from_bits(bits).to_bits(), bits);
    }
}

#[test]
fn binary64_bits_round_trip() {
    for bits in [
        0,
        1,
        0x8000_0000_0000_0000,
        0x7ff0_0000_0000_0000,
        0xffff_ffff_ffff_ffff,
    ] {
        assert_eq!(Float64::from_bits(bits).to_bits(), bits);
    }
}

#[test]
fn binary32_classification_uses_legacy_mips_nan_convention() {
    for bits in [
        0x0000_0000,
        0x8000_0000,
        0x0080_0000,
        0x8080_0000,
        0x7f80_0000,
        0xff80_0000,
    ] {
        let value = Float32::from_bits(bits);
        assert!(!value.is_nan());
        assert!(!value.is_signaling_nan());
        assert!(!value.is_subnormal());
    }

    for bits in [0x0000_0001, 0x007f_ffff, 0x8000_0001, 0x807f_ffff] {
        let value = Float32::from_bits(bits);
        assert!(!value.is_nan());
        assert!(!value.is_signaling_nan());
        assert!(value.is_subnormal());
    }

    for bits in [0x7fbf_ffff, 0xffbf_ffff] {
        let value = Float32::from_bits(bits);
        assert!(value.is_nan());
        assert!(!value.is_signaling_nan());
        assert!(!value.is_subnormal());
    }

    for bits in [0x7fc0_0000, 0xffc0_0000] {
        let value = Float32::from_bits(bits);
        assert!(value.is_nan());
        assert!(value.is_signaling_nan());
        assert!(!value.is_subnormal());
    }
}

#[test]
fn binary64_classification_uses_legacy_mips_nan_convention() {
    for bits in [
        0x0000_0000_0000_0000,
        0x8000_0000_0000_0000,
        0x0010_0000_0000_0000,
        0x8010_0000_0000_0000,
        0x7ff0_0000_0000_0000,
        0xfff0_0000_0000_0000,
    ] {
        let value = Float64::from_bits(bits);
        assert!(!value.is_nan());
        assert!(!value.is_signaling_nan());
        assert!(!value.is_subnormal());
    }

    for bits in [
        0x0000_0000_0000_0001,
        0x000f_ffff_ffff_ffff,
        0x8000_0000_0000_0001,
        0x800f_ffff_ffff_ffff,
    ] {
        let value = Float64::from_bits(bits);
        assert!(!value.is_nan());
        assert!(!value.is_signaling_nan());
        assert!(value.is_subnormal());
    }

    for bits in [0x7ff7_ffff_ffff_ffff, 0xfff7_ffff_ffff_ffff] {
        let value = Float64::from_bits(bits);
        assert!(value.is_nan());
        assert!(!value.is_signaling_nan());
        assert!(!value.is_subnormal());
    }

    for bits in [0x7ff8_0000_0000_0000, 0xfff8_0000_0000_0000] {
        let value = Float64::from_bits(bits);
        assert!(value.is_nan());
        assert!(value.is_signaling_nan());
        assert!(!value.is_subnormal());
    }
}
