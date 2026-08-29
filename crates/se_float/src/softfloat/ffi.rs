unsafe extern "C" {
    pub(super) fn se_float_softfloat_add_f32(
        lhs: u32,
        rhs: u32,
        rounding: u8,
        exception_flags: *mut u8,
    ) -> u32;
    pub(super) fn se_float_softfloat_add_f64(
        lhs: u64,
        rhs: u64,
        rounding: u8,
        exception_flags: *mut u8,
    ) -> u64;
    pub(super) fn se_float_softfloat_sub_f32(
        lhs: u32,
        rhs: u32,
        rounding: u8,
        exception_flags: *mut u8,
    ) -> u32;
    pub(super) fn se_float_softfloat_sub_f64(
        lhs: u64,
        rhs: u64,
        rounding: u8,
        exception_flags: *mut u8,
    ) -> u64;
    pub(super) fn se_float_softfloat_mul_f32(
        lhs: u32,
        rhs: u32,
        rounding: u8,
        exception_flags: *mut u8,
    ) -> u32;
    pub(super) fn se_float_softfloat_mul_f64(
        lhs: u64,
        rhs: u64,
        rounding: u8,
        exception_flags: *mut u8,
    ) -> u64;
    pub(super) fn se_float_softfloat_div_f32(
        lhs: u32,
        rhs: u32,
        rounding: u8,
        exception_flags: *mut u8,
    ) -> u32;
    pub(super) fn se_float_softfloat_div_f64(
        lhs: u64,
        rhs: u64,
        rounding: u8,
        exception_flags: *mut u8,
    ) -> u64;
    pub(super) fn se_float_softfloat_convert_f32_to_f64(
        value: u32,
        exception_flags: *mut u8,
    ) -> u64;
    pub(super) fn se_float_softfloat_convert_f64_to_f32(
        value: u64,
        rounding: u8,
        exception_flags: *mut u8,
    ) -> u32;
    pub(super) fn se_float_softfloat_convert_i32_to_f32(
        value: i32,
        rounding: u8,
        exception_flags: *mut u8,
    ) -> u32;
    pub(super) fn se_float_softfloat_convert_i32_to_f64(value: i32) -> u64;
    pub(super) fn se_float_softfloat_convert_f32_to_i32(
        value: u32,
        rounding: u8,
        exception_flags: *mut u8,
    ) -> i32;
    pub(super) fn se_float_softfloat_convert_f64_to_i32(
        value: u64,
        rounding: u8,
        exception_flags: *mut u8,
    ) -> i32;
    pub(super) fn se_float_softfloat_compare_f32(
        lhs: u32,
        rhs: u32,
        signaling: u8,
        exception_flags: *mut u8,
    ) -> u8;
    pub(super) fn se_float_softfloat_compare_f64(
        lhs: u64,
        rhs: u64,
        signaling: u8,
        exception_flags: *mut u8,
    ) -> u8;
}
