//! Classifies IEEE binary32 and binary64 raw bit patterns.
//!
//! These helpers describe only standard IEEE encodings. Guest NaN polarity,
//! register formats, and denormal policies remain outside this module.

#[cfg(test)]
pub(crate) const fn is_nonzero_subnormal_f32(bits: u32) -> bool {
    bits & 0x7f80_0000 == 0 && bits & 0x007f_ffff != 0
}

#[cfg(test)]
pub(crate) const fn is_nonzero_subnormal_f64(bits: u64) -> bool {
    bits & 0x7ff0_0000_0000_0000 == 0 && bits & 0x000f_ffff_ffff_ffff != 0
}

#[cfg(test)]
const fn is_nan_f32(bits: u32) -> bool {
    bits & 0x7f80_0000 == 0x7f80_0000 && bits & 0x007f_ffff != 0
}

#[cfg(test)]
const fn is_nan_f64(bits: u64) -> bool {
    bits & 0x7ff0_0000_0000_0000 == 0x7ff0_0000_0000_0000 && bits & 0x000f_ffff_ffff_ffff != 0
}

#[cfg(test)]
mod tests {
    use super::{is_nan_f32, is_nan_f64, is_nonzero_subnormal_f32, is_nonzero_subnormal_f64};

    #[test]
    fn classifies_binary32_boundaries() {
        assert!(!is_nonzero_subnormal_f32(0));
        assert!(is_nonzero_subnormal_f32(1));
        assert!(is_nonzero_subnormal_f32(0x807f_ffff));
        assert!(!is_nonzero_subnormal_f32(0x0080_0000));
        assert!(is_nan_f32(0x7fc0_0000));
        assert!(!is_nan_f32(0x7f80_0000));
    }

    #[test]
    fn classifies_binary64_boundaries() {
        assert!(!is_nonzero_subnormal_f64(0));
        assert!(is_nonzero_subnormal_f64(1));
        assert!(is_nonzero_subnormal_f64(0x800f_ffff_ffff_ffff));
        assert!(!is_nonzero_subnormal_f64(0x0010_0000_0000_0000));
        assert!(is_nan_f64(0x7ff8_0000_0000_0000));
        assert!(!is_nan_f64(0x7ff0_0000_0000_0000));
    }
}
