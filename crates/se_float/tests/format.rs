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
