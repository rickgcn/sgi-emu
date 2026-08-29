use crate::format::{Float32, Float64};
use crate::operation::{ComparisonMode, ExceptionFlags, Outcome, Relation, RoundingMode};

pub(super) fn add_f32(lhs: Float32, rhs: Float32, _rounding: RoundingMode) -> Outcome<Float32> {
    rounded_f32(f32::from_bits(lhs.to_bits()) + f32::from_bits(rhs.to_bits()))
}

pub(super) fn add_f64(lhs: Float64, rhs: Float64, _rounding: RoundingMode) -> Outcome<Float64> {
    rounded_f64(f64::from_bits(lhs.to_bits()) + f64::from_bits(rhs.to_bits()))
}

pub(super) fn sub_f32(lhs: Float32, rhs: Float32, _rounding: RoundingMode) -> Outcome<Float32> {
    rounded_f32(f32::from_bits(lhs.to_bits()) - f32::from_bits(rhs.to_bits()))
}

pub(super) fn sub_f64(lhs: Float64, rhs: Float64, _rounding: RoundingMode) -> Outcome<Float64> {
    rounded_f64(f64::from_bits(lhs.to_bits()) - f64::from_bits(rhs.to_bits()))
}

pub(super) fn mul_f32(lhs: Float32, rhs: Float32, _rounding: RoundingMode) -> Outcome<Float32> {
    rounded_f32(f32::from_bits(lhs.to_bits()) * f32::from_bits(rhs.to_bits()))
}

pub(super) fn mul_f64(lhs: Float64, rhs: Float64, _rounding: RoundingMode) -> Outcome<Float64> {
    rounded_f64(f64::from_bits(lhs.to_bits()) * f64::from_bits(rhs.to_bits()))
}

pub(super) fn div_f32(lhs: Float32, rhs: Float32, _rounding: RoundingMode) -> Outcome<Float32> {
    rounded_f32(f32::from_bits(lhs.to_bits()) / f32::from_bits(rhs.to_bits()))
}

pub(super) fn div_f64(lhs: Float64, rhs: Float64, _rounding: RoundingMode) -> Outcome<Float64> {
    rounded_f64(f64::from_bits(lhs.to_bits()) / f64::from_bits(rhs.to_bits()))
}

pub(super) fn abs_f32(value: Float32) -> Outcome<Float32> {
    plain_f32(f32::from_bits(value.to_bits()).abs())
}

pub(super) fn abs_f64(value: Float64) -> Outcome<Float64> {
    plain_f64(f64::from_bits(value.to_bits()).abs())
}

pub(super) fn neg_f32(value: Float32) -> Outcome<Float32> {
    plain_f32(-f32::from_bits(value.to_bits()))
}

pub(super) fn neg_f64(value: Float64) -> Outcome<Float64> {
    plain_f64(-f64::from_bits(value.to_bits()))
}

pub(super) fn convert_float32_to_float64(value: Float32) -> Outcome<Float64> {
    plain_f64(f32::from_bits(value.to_bits()) as f64)
}

pub(super) fn convert_float64_to_float32(
    value: Float64,
    _rounding: RoundingMode,
) -> Outcome<Float32> {
    rounded_f32(f64::from_bits(value.to_bits()) as f32)
}

pub(super) fn convert_i32_to_float32(value: i32, _rounding: RoundingMode) -> Outcome<Float32> {
    plain_f32(value as f32)
}

pub(super) fn convert_i32_to_float64(value: i32) -> Outcome<Float64> {
    plain_f64(value as f64)
}

pub(super) fn convert_float32_to_i32(value: Float32, _rounding: RoundingMode) -> Outcome<i32> {
    plain_i32(f32::from_bits(value.to_bits()) as i32)
}

pub(super) fn convert_float64_to_i32(value: Float64, _rounding: RoundingMode) -> Outcome<i32> {
    plain_i32(f64::from_bits(value.to_bits()) as i32)
}

pub(super) fn compare_f32(lhs: Float32, rhs: Float32, _mode: ComparisonMode) -> Outcome<Relation> {
    let lhs = f32::from_bits(lhs.to_bits());
    let rhs = f32::from_bits(rhs.to_bits());
    plain_relation(if lhs.is_nan() || rhs.is_nan() {
        Relation::Unordered
    } else if lhs < rhs {
        Relation::Less
    } else if lhs > rhs {
        Relation::Greater
    } else {
        Relation::Equal
    })
}

pub(super) fn compare_f64(lhs: Float64, rhs: Float64, _mode: ComparisonMode) -> Outcome<Relation> {
    let lhs = f64::from_bits(lhs.to_bits());
    let rhs = f64::from_bits(rhs.to_bits());
    plain_relation(if lhs.is_nan() || rhs.is_nan() {
        Relation::Unordered
    } else if lhs < rhs {
        Relation::Less
    } else if lhs > rhs {
        Relation::Greater
    } else {
        Relation::Equal
    })
}

fn rounded_f32(value: f32) -> Outcome<Float32> {
    Outcome {
        value: Float32::from_bits(value.to_bits()),
        flags: ExceptionFlags::empty(),
        tiny: value.is_subnormal(),
    }
}

fn rounded_f64(value: f64) -> Outcome<Float64> {
    Outcome {
        value: Float64::from_bits(value.to_bits()),
        flags: ExceptionFlags::empty(),
        tiny: value.is_subnormal(),
    }
}

fn plain_f32(value: f32) -> Outcome<Float32> {
    Outcome {
        value: Float32::from_bits(value.to_bits()),
        flags: ExceptionFlags::empty(),
        tiny: false,
    }
}

fn plain_f64(value: f64) -> Outcome<Float64> {
    Outcome {
        value: Float64::from_bits(value.to_bits()),
        flags: ExceptionFlags::empty(),
        tiny: false,
    }
}

fn plain_i32(value: i32) -> Outcome<i32> {
    Outcome {
        value,
        flags: ExceptionFlags::empty(),
        tiny: false,
    }
}

fn plain_relation(value: Relation) -> Outcome<Relation> {
    Outcome {
        value,
        flags: ExceptionFlags::empty(),
        tiny: false,
    }
}
