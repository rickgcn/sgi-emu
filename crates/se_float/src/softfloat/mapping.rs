use crate::operation::{ExceptionFlags, Relation, RoundingMode};

pub(super) const fn rounding_mode(mode: RoundingMode) -> u8 {
    match mode {
        RoundingMode::NearestEven => 0,
        RoundingMode::TowardZero => 1,
        RoundingMode::TowardNegative => 2,
        RoundingMode::TowardPositive => 3,
    }
}

pub(super) fn exception_flags(bits: u8) -> ExceptionFlags {
    match ExceptionFlags::from_bits(bits) {
        Some(flags) => flags,
        None => panic!("SoftFloat bridge returned unknown exception flags: {bits:#04x}"),
    }
}

pub(super) const fn relation(value: u8) -> Relation {
    match value {
        0 => Relation::Less,
        1 => Relation::Equal,
        2 => Relation::Greater,
        3 => Relation::Unordered,
        _ => unreachable!(),
    }
}

pub(super) const fn is_subnormal_f32(bits: u32) -> bool {
    bits & 0x7f80_0000 == 0 && bits & 0x007f_ffff != 0
}

pub(super) const fn is_subnormal_f64(bits: u64) -> bool {
    bits & 0x7ff0_0000_0000_0000 == 0 && bits & 0x000f_ffff_ffff_ffff != 0
}
