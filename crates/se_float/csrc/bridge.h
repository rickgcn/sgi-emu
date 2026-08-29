// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef SE_FLOAT_SOFTFLOAT_BRIDGE_H
#define SE_FLOAT_SOFTFLOAT_BRIDGE_H

#include <stdint.h>

#define SE_FLOAT_SOFTFLOAT_ROUND_NEAREST_EVEN 0
#define SE_FLOAT_SOFTFLOAT_ROUND_TOWARD_ZERO 1
#define SE_FLOAT_SOFTFLOAT_ROUND_TOWARD_NEGATIVE 2
#define SE_FLOAT_SOFTFLOAT_ROUND_TOWARD_POSITIVE 3

#define SE_FLOAT_SOFTFLOAT_RELATION_LESS 0
#define SE_FLOAT_SOFTFLOAT_RELATION_EQUAL 1
#define SE_FLOAT_SOFTFLOAT_RELATION_GREATER 2
#define SE_FLOAT_SOFTFLOAT_RELATION_UNORDERED 3

#define SE_FLOAT_SOFTFLOAT_FLAG_INEXACT (1 << 0)
#define SE_FLOAT_SOFTFLOAT_FLAG_UNDERFLOW (1 << 1)
#define SE_FLOAT_SOFTFLOAT_FLAG_OVERFLOW (1 << 2)
#define SE_FLOAT_SOFTFLOAT_FLAG_DIVIDE_BY_ZERO (1 << 3)
#define SE_FLOAT_SOFTFLOAT_FLAG_INVALID (1 << 4)

uint32_t se_float_softfloat_add_f32(
    uint32_t lhs,
    uint32_t rhs,
    uint8_t rounding,
    uint8_t *exception_flags
);
uint64_t se_float_softfloat_add_f64(
    uint64_t lhs,
    uint64_t rhs,
    uint8_t rounding,
    uint8_t *exception_flags
);
uint32_t se_float_softfloat_sub_f32(
    uint32_t lhs,
    uint32_t rhs,
    uint8_t rounding,
    uint8_t *exception_flags
);
uint64_t se_float_softfloat_sub_f64(
    uint64_t lhs,
    uint64_t rhs,
    uint8_t rounding,
    uint8_t *exception_flags
);
uint32_t se_float_softfloat_mul_f32(
    uint32_t lhs,
    uint32_t rhs,
    uint8_t rounding,
    uint8_t *exception_flags
);
uint64_t se_float_softfloat_mul_f64(
    uint64_t lhs,
    uint64_t rhs,
    uint8_t rounding,
    uint8_t *exception_flags
);
uint32_t se_float_softfloat_div_f32(
    uint32_t lhs,
    uint32_t rhs,
    uint8_t rounding,
    uint8_t *exception_flags
);
uint64_t se_float_softfloat_div_f64(
    uint64_t lhs,
    uint64_t rhs,
    uint8_t rounding,
    uint8_t *exception_flags
);

uint64_t se_float_softfloat_convert_f32_to_f64(
    uint32_t value,
    uint8_t *exception_flags
);
uint32_t se_float_softfloat_convert_f64_to_f32(
    uint64_t value,
    uint8_t rounding,
    uint8_t *exception_flags
);
uint32_t se_float_softfloat_convert_i32_to_f32(
    int32_t value,
    uint8_t rounding,
    uint8_t *exception_flags
);
uint64_t se_float_softfloat_convert_i32_to_f64(int32_t value);
int32_t se_float_softfloat_convert_f32_to_i32(
    uint32_t value,
    uint8_t rounding,
    uint8_t *exception_flags
);
int32_t se_float_softfloat_convert_f64_to_i32(
    uint64_t value,
    uint8_t rounding,
    uint8_t *exception_flags
);

uint8_t se_float_softfloat_compare_f32(
    uint32_t lhs,
    uint32_t rhs,
    uint8_t signaling,
    uint8_t *exception_flags
);
uint8_t se_float_softfloat_compare_f64(
    uint64_t lhs,
    uint64_t rhs,
    uint8_t signaling,
    uint8_t *exception_flags
);

#endif
