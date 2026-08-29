// SPDX-License-Identifier: GPL-3.0-or-later

#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

#include "platform.h"
#include "softfloat.h"
#include "bridge.h"

static float32_t float32_from_bits(uint32_t bits)
{
    float32_t value;
    value.v = bits;
    return value;
}

static float64_t float64_from_bits(uint64_t bits)
{
    float64_t value;
    value.v = bits;
    return value;
}

static uint_fast8_t softfloat_rounding(uint8_t rounding)
{
    switch (rounding) {
    case SE_FLOAT_SOFTFLOAT_ROUND_NEAREST_EVEN:
        return softfloat_round_near_even;
    case SE_FLOAT_SOFTFLOAT_ROUND_TOWARD_ZERO:
        return softfloat_round_minMag;
    case SE_FLOAT_SOFTFLOAT_ROUND_TOWARD_NEGATIVE:
        return softfloat_round_min;
    case SE_FLOAT_SOFTFLOAT_ROUND_TOWARD_POSITIVE:
        return softfloat_round_max;
    default:
        abort();
    }
}

static void begin_operation(uint8_t rounding)
{
    softfloat_roundingMode = softfloat_rounding(rounding);
    softfloat_detectTininess = softfloat_tininess_afterRounding;
    softfloat_exceptionFlags = 0;
}

static void finish_operation(uint8_t *exception_flags)
{
    uint8_t flags = 0;

    if (softfloat_exceptionFlags & softfloat_flag_inexact) {
        flags |= SE_FLOAT_SOFTFLOAT_FLAG_INEXACT;
    }
    if (softfloat_exceptionFlags & softfloat_flag_underflow) {
        flags |= SE_FLOAT_SOFTFLOAT_FLAG_UNDERFLOW;
    }
    if (softfloat_exceptionFlags & softfloat_flag_overflow) {
        flags |= SE_FLOAT_SOFTFLOAT_FLAG_OVERFLOW;
    }
    if (softfloat_exceptionFlags & softfloat_flag_infinite) {
        flags |= SE_FLOAT_SOFTFLOAT_FLAG_DIVIDE_BY_ZERO;
    }
    if (softfloat_exceptionFlags & softfloat_flag_invalid) {
        flags |= SE_FLOAT_SOFTFLOAT_FLAG_INVALID;
    }

    *exception_flags = flags;
}

uint32_t se_float_softfloat_add_f32(
    uint32_t lhs,
    uint32_t rhs,
    uint8_t rounding,
    uint8_t *exception_flags
)
{
    float32_t result;
    begin_operation(rounding);
    result = f32_add(float32_from_bits(lhs), float32_from_bits(rhs));
    finish_operation(exception_flags);
    return result.v;
}

uint64_t se_float_softfloat_add_f64(
    uint64_t lhs,
    uint64_t rhs,
    uint8_t rounding,
    uint8_t *exception_flags
)
{
    float64_t result;
    begin_operation(rounding);
    result = f64_add(float64_from_bits(lhs), float64_from_bits(rhs));
    finish_operation(exception_flags);
    return result.v;
}

uint32_t se_float_softfloat_sub_f32(
    uint32_t lhs,
    uint32_t rhs,
    uint8_t rounding,
    uint8_t *exception_flags
)
{
    float32_t result;
    begin_operation(rounding);
    result = f32_sub(float32_from_bits(lhs), float32_from_bits(rhs));
    finish_operation(exception_flags);
    return result.v;
}

uint64_t se_float_softfloat_sub_f64(
    uint64_t lhs,
    uint64_t rhs,
    uint8_t rounding,
    uint8_t *exception_flags
)
{
    float64_t result;
    begin_operation(rounding);
    result = f64_sub(float64_from_bits(lhs), float64_from_bits(rhs));
    finish_operation(exception_flags);
    return result.v;
}

uint32_t se_float_softfloat_mul_f32(
    uint32_t lhs,
    uint32_t rhs,
    uint8_t rounding,
    uint8_t *exception_flags
)
{
    float32_t result;
    begin_operation(rounding);
    result = f32_mul(float32_from_bits(lhs), float32_from_bits(rhs));
    finish_operation(exception_flags);
    return result.v;
}

uint64_t se_float_softfloat_mul_f64(
    uint64_t lhs,
    uint64_t rhs,
    uint8_t rounding,
    uint8_t *exception_flags
)
{
    float64_t result;
    begin_operation(rounding);
    result = f64_mul(float64_from_bits(lhs), float64_from_bits(rhs));
    finish_operation(exception_flags);
    return result.v;
}

uint32_t se_float_softfloat_div_f32(
    uint32_t lhs,
    uint32_t rhs,
    uint8_t rounding,
    uint8_t *exception_flags
)
{
    float32_t result;
    begin_operation(rounding);
    result = f32_div(float32_from_bits(lhs), float32_from_bits(rhs));
    finish_operation(exception_flags);
    return result.v;
}

uint64_t se_float_softfloat_div_f64(
    uint64_t lhs,
    uint64_t rhs,
    uint8_t rounding,
    uint8_t *exception_flags
)
{
    float64_t result;
    begin_operation(rounding);
    result = f64_div(float64_from_bits(lhs), float64_from_bits(rhs));
    finish_operation(exception_flags);
    return result.v;
}

uint64_t se_float_softfloat_convert_f32_to_f64(
    uint32_t value,
    uint8_t *exception_flags
)
{
    float64_t result;
    begin_operation(SE_FLOAT_SOFTFLOAT_ROUND_NEAREST_EVEN);
    result = f32_to_f64(float32_from_bits(value));
    finish_operation(exception_flags);
    return result.v;
}

uint32_t se_float_softfloat_convert_f64_to_f32(
    uint64_t value,
    uint8_t rounding,
    uint8_t *exception_flags
)
{
    float32_t result;
    begin_operation(rounding);
    result = f64_to_f32(float64_from_bits(value));
    finish_operation(exception_flags);
    return result.v;
}

uint32_t se_float_softfloat_convert_i32_to_f32(
    int32_t value,
    uint8_t rounding,
    uint8_t *exception_flags
)
{
    float32_t result;
    begin_operation(rounding);
    result = i32_to_f32(value);
    finish_operation(exception_flags);
    return result.v;
}

uint64_t se_float_softfloat_convert_i32_to_f64(int32_t value)
{
    return i32_to_f64(value).v;
}

int32_t se_float_softfloat_convert_f32_to_i32(
    uint32_t value,
    uint8_t rounding,
    uint8_t *exception_flags
)
{
    int_fast32_t result;
    begin_operation(rounding);
    result = f32_to_i32(float32_from_bits(value), softfloat_roundingMode, true);
    finish_operation(exception_flags);
    return (int32_t)result;
}

int32_t se_float_softfloat_convert_f64_to_i32(
    uint64_t value,
    uint8_t rounding,
    uint8_t *exception_flags
)
{
    int_fast32_t result;
    begin_operation(rounding);
    result = f64_to_i32(float64_from_bits(value), softfloat_roundingMode, true);
    finish_operation(exception_flags);
    return (int32_t)result;
}

uint8_t se_float_softfloat_compare_f32(
    uint32_t lhs,
    uint32_t rhs,
    uint8_t signaling,
    uint8_t *exception_flags
)
{
    float32_t lhs_value = float32_from_bits(lhs);
    float32_t rhs_value = float32_from_bits(rhs);
    uint8_t relation;

    begin_operation(SE_FLOAT_SOFTFLOAT_ROUND_NEAREST_EVEN);
    if (signaling ? f32_eq_signaling(lhs_value, rhs_value) : f32_eq(lhs_value, rhs_value)) {
        relation = SE_FLOAT_SOFTFLOAT_RELATION_EQUAL;
    } else if (f32_lt_quiet(lhs_value, rhs_value)) {
        relation = SE_FLOAT_SOFTFLOAT_RELATION_LESS;
    } else if (f32_lt_quiet(rhs_value, lhs_value)) {
        relation = SE_FLOAT_SOFTFLOAT_RELATION_GREATER;
    } else {
        relation = SE_FLOAT_SOFTFLOAT_RELATION_UNORDERED;
    }
    finish_operation(exception_flags);
    return relation;
}

uint8_t se_float_softfloat_compare_f64(
    uint64_t lhs,
    uint64_t rhs,
    uint8_t signaling,
    uint8_t *exception_flags
)
{
    float64_t lhs_value = float64_from_bits(lhs);
    float64_t rhs_value = float64_from_bits(rhs);
    uint8_t relation;

    begin_operation(SE_FLOAT_SOFTFLOAT_ROUND_NEAREST_EVEN);
    if (signaling ? f64_eq_signaling(lhs_value, rhs_value) : f64_eq(lhs_value, rhs_value)) {
        relation = SE_FLOAT_SOFTFLOAT_RELATION_EQUAL;
    } else if (f64_lt_quiet(lhs_value, rhs_value)) {
        relation = SE_FLOAT_SOFTFLOAT_RELATION_LESS;
    } else if (f64_lt_quiet(rhs_value, lhs_value)) {
        relation = SE_FLOAT_SOFTFLOAT_RELATION_GREATER;
    } else {
        relation = SE_FLOAT_SOFTFLOAT_RELATION_UNORDERED;
    }
    finish_operation(exception_flags);
    return relation;
}

