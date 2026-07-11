#include <stdbool.h>
#include <stdint.h>
#include "platform.h"
#include "softfloat.h"

static float32_t se_f32(uint32_t bits)
{
    float32_t value;
    value.v = bits;
    return value;
}

static float64_t se_f64(uint64_t bits)
{
    float64_t value;
    value.v = bits;
    return value;
}

void se_softfloat_set_rounding_mode(uint8_t value)
{
    softfloat_roundingMode = value;
}

void se_softfloat_set_tininess_mode(uint8_t value)
{
    softfloat_detectTininess = value;
}

void se_softfloat_clear_exception_flags(void)
{
    softfloat_exceptionFlags = 0;
}

uint8_t se_softfloat_exception_flags(void)
{
    return softfloat_exceptionFlags;
}

uint32_t se_softfloat_f32_add(uint32_t lhs, uint32_t rhs)
{
    return f32_add(se_f32(lhs), se_f32(rhs)).v;
}

uint32_t se_softfloat_f32_sub(uint32_t lhs, uint32_t rhs)
{
    return f32_sub(se_f32(lhs), se_f32(rhs)).v;
}

uint32_t se_softfloat_f32_mul(uint32_t lhs, uint32_t rhs)
{
    return f32_mul(se_f32(lhs), se_f32(rhs)).v;
}

uint32_t se_softfloat_f32_div(uint32_t lhs, uint32_t rhs)
{
    return f32_div(se_f32(lhs), se_f32(rhs)).v;
}

uint32_t se_softfloat_f32_sqrt(uint32_t value)
{
    return f32_sqrt(se_f32(value)).v;
}

uint32_t se_softfloat_f32_mul_add(uint32_t lhs, uint32_t rhs, uint32_t addend)
{
    return f32_mulAdd(se_f32(lhs), se_f32(rhs), se_f32(addend)).v;
}

uint64_t se_softfloat_f64_add(uint64_t lhs, uint64_t rhs)
{
    return f64_add(se_f64(lhs), se_f64(rhs)).v;
}

uint64_t se_softfloat_f64_sub(uint64_t lhs, uint64_t rhs)
{
    return f64_sub(se_f64(lhs), se_f64(rhs)).v;
}

uint64_t se_softfloat_f64_mul(uint64_t lhs, uint64_t rhs)
{
    return f64_mul(se_f64(lhs), se_f64(rhs)).v;
}

uint64_t se_softfloat_f64_div(uint64_t lhs, uint64_t rhs)
{
    return f64_div(se_f64(lhs), se_f64(rhs)).v;
}

uint64_t se_softfloat_f64_sqrt(uint64_t value)
{
    return f64_sqrt(se_f64(value)).v;
}

uint64_t se_softfloat_f64_mul_add(uint64_t lhs, uint64_t rhs, uint64_t addend)
{
    return f64_mulAdd(se_f64(lhs), se_f64(rhs), se_f64(addend)).v;
}

uint64_t se_softfloat_f32_to_f64(uint32_t value)
{
    return f32_to_f64(se_f32(value)).v;
}

uint32_t se_softfloat_f64_to_f32(uint64_t value)
{
    return f64_to_f32(se_f64(value)).v;
}

uint32_t se_softfloat_i32_to_f32(int32_t value)
{
    return i32_to_f32(value).v;
}

uint64_t se_softfloat_i32_to_f64(int32_t value)
{
    return i32_to_f64(value).v;
}

uint32_t se_softfloat_i64_to_f32(int64_t value)
{
    return i64_to_f32(value).v;
}

uint64_t se_softfloat_i64_to_f64(int64_t value)
{
    return i64_to_f64(value).v;
}

int32_t se_softfloat_f32_to_i32(uint32_t value, uint8_t rounding_mode, bool exact)
{
    return (int32_t) f32_to_i32(se_f32(value), rounding_mode, exact);
}

int32_t se_softfloat_f64_to_i32(uint64_t value, uint8_t rounding_mode, bool exact)
{
    return (int32_t) f64_to_i32(se_f64(value), rounding_mode, exact);
}

int64_t se_softfloat_f32_to_i64(uint32_t value, uint8_t rounding_mode, bool exact)
{
    return f32_to_i64(se_f32(value), rounding_mode, exact);
}

int64_t se_softfloat_f64_to_i64(uint64_t value, uint8_t rounding_mode, bool exact)
{
    return f64_to_i64(se_f64(value), rounding_mode, exact);
}

bool se_softfloat_f32_eq(uint32_t lhs, uint32_t rhs)
{
    return f32_eq(se_f32(lhs), se_f32(rhs));
}

bool se_softfloat_f32_eq_signaling(uint32_t lhs, uint32_t rhs)
{
    return f32_eq_signaling(se_f32(lhs), se_f32(rhs));
}

bool se_softfloat_f32_lt(uint32_t lhs, uint32_t rhs)
{
    return f32_lt(se_f32(lhs), se_f32(rhs));
}

bool se_softfloat_f64_eq(uint64_t lhs, uint64_t rhs)
{
    return f64_eq(se_f64(lhs), se_f64(rhs));
}

bool se_softfloat_f64_eq_signaling(uint64_t lhs, uint64_t rhs)
{
    return f64_eq_signaling(se_f64(lhs), se_f64(rhs));
}

bool se_softfloat_f64_lt(uint64_t lhs, uint64_t rhs)
{
    return f64_lt(se_f64(lhs), se_f64(rhs));
}
