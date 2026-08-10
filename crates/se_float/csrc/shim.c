#include <stdbool.h>
#include <stdint.h>
#include "platform.h"
#include "internals.h"
#include "round_pack.h"
#include "shim.h"
#include "softfloat.h"

#define SE_FLOAT_SHIM_INVALID_FLAGS UINT8_C(0x80)
#define SE_FLOAT_SHIM_INVALID_FACT UINT8_C(2)

static void se_float_shim_contract_failure(uint8_t *out_flags,
                                           uint8_t *out_precision_inexact) {
    *out_flags = SE_FLOAT_SHIM_INVALID_FLAGS;
    *out_precision_inexact = SE_FLOAT_SHIM_INVALID_FACT;
}

static uint8_t se_float_shim_begin(uint8_t rounding, uint8_t *out_flags,
                                   uint8_t *out_precision_inexact) {
    switch (rounding) {
    case 0:
        softfloat_roundingMode = softfloat_round_near_even;
        break;
    case 1:
        softfloat_roundingMode = softfloat_round_minMag;
        break;
    case 2:
        softfloat_roundingMode = softfloat_round_max;
        break;
    case 3:
        softfloat_roundingMode = softfloat_round_min;
        break;
    default:
        se_float_shim_contract_failure(out_flags, out_precision_inexact);
        return 0;
    }

    softfloat_detectTininess = softfloat_tininess_afterRounding;
    softfloat_exceptionFlags = 0;
    se_float_sf_roundPackReset();
    return 1;
}

static void se_float_shim_finish_scalar(uint8_t *out_flags,
                                        uint8_t *out_precision_inexact) {
    *out_flags = softfloat_exceptionFlags;
    *out_precision_inexact =
        (softfloat_exceptionFlags & softfloat_flag_inexact) != 0;
}

static void se_float_shim_finish_relation(uint8_t *out_flags,
                                          uint8_t *out_precision_inexact) {
    *out_flags = softfloat_exceptionFlags;
    *out_precision_inexact = 0;
}

static void se_float_shim_finish_f32(uint8_t *out_flags,
                                     uint8_t *out_precision_inexact) {
    uint8_t precision_inexact;

    if (softfloat_exceptionFlags & softfloat_flag_overflow) {
        precision_inexact = se_float_sf_roundPackToF32PrecisionInexact();
        if (1 < precision_inexact) {
            se_float_shim_contract_failure(out_flags, out_precision_inexact);
            return;
        }
    } else {
        precision_inexact =
            (softfloat_exceptionFlags & softfloat_flag_inexact) != 0;
    }
    *out_flags = softfloat_exceptionFlags;
    *out_precision_inexact = precision_inexact;
}

static void se_float_shim_finish_f64(uint8_t *out_flags,
                                     uint8_t *out_precision_inexact) {
    uint8_t precision_inexact;

    if (softfloat_exceptionFlags & softfloat_flag_overflow) {
        precision_inexact = se_float_sf_roundPackToF64PrecisionInexact();
        if (1 < precision_inexact) {
            se_float_shim_contract_failure(out_flags, out_precision_inexact);
            return;
        }
    } else {
        precision_inexact =
            (softfloat_exceptionFlags & softfloat_flag_inexact) != 0;
    }
    *out_flags = softfloat_exceptionFlags;
    *out_precision_inexact = precision_inexact;
}

static uint8_t se_float_shim_is_nan_f32(uint32_t value) {
    return ((value & UINT32_C(0x7F800000)) == UINT32_C(0x7F800000)) &&
           (value & UINT32_C(0x007FFFFF));
}

static uint8_t se_float_shim_is_nan_f64(uint64_t value) {
    return ((value & UINT64_C(0x7FF0000000000000)) ==
            UINT64_C(0x7FF0000000000000)) &&
           (value & UINT64_C(0x000FFFFFFFFFFFFF));
}

uint32_t se_float_shim_add_f32(uint32_t a, uint32_t b, uint8_t rounding,
                               uint8_t *out_flags,
                               uint8_t *out_precision_inexact) {
    float32_t result;

    if (!se_float_shim_begin(rounding, out_flags, out_precision_inexact))
        return 0;
    result = f32_add((float32_t){a}, (float32_t){b});
    se_float_shim_finish_f32(out_flags, out_precision_inexact);
    return result.v;
}

uint32_t se_float_shim_sub_f32(uint32_t a, uint32_t b, uint8_t rounding,
                               uint8_t *out_flags,
                               uint8_t *out_precision_inexact) {
    float32_t result;

    if (!se_float_shim_begin(rounding, out_flags, out_precision_inexact))
        return 0;
    result = f32_sub((float32_t){a}, (float32_t){b});
    se_float_shim_finish_f32(out_flags, out_precision_inexact);
    return result.v;
}

uint32_t se_float_shim_mul_f32(uint32_t a, uint32_t b, uint8_t rounding,
                               uint8_t *out_flags,
                               uint8_t *out_precision_inexact) {
    float32_t result;

    if (!se_float_shim_begin(rounding, out_flags, out_precision_inexact))
        return 0;
    result = f32_mul((float32_t){a}, (float32_t){b});
    se_float_shim_finish_f32(out_flags, out_precision_inexact);
    return result.v;
}

uint32_t se_float_shim_div_f32(uint32_t a, uint32_t b, uint8_t rounding,
                               uint8_t *out_flags,
                               uint8_t *out_precision_inexact) {
    float32_t result;

    if (!se_float_shim_begin(rounding, out_flags, out_precision_inexact))
        return 0;
    result = f32_div((float32_t){a}, (float32_t){b});
    se_float_shim_finish_f32(out_flags, out_precision_inexact);
    return result.v;
}

uint32_t se_float_shim_sqrt_f32(uint32_t value, uint8_t rounding,
                                uint8_t *out_flags,
                                uint8_t *out_precision_inexact) {
    float32_t result;

    if (!se_float_shim_begin(rounding, out_flags, out_precision_inexact))
        return 0;
    result = f32_sqrt((float32_t){value});
    se_float_shim_finish_f32(out_flags, out_precision_inexact);
    return result.v;
}

uint8_t se_float_shim_compare_f32(uint32_t a, uint32_t b,
                                  uint8_t *out_flags,
                                  uint8_t *out_precision_inexact) {
    uint8_t relation;

    if (!se_float_shim_begin(0, out_flags, out_precision_inexact))
        return 0;
    if (f32_eq((float32_t){a}, (float32_t){b})) {
        relation = 1;
    } else if (se_float_shim_is_nan_f32(a) ||
               se_float_shim_is_nan_f32(b)) {
        relation = 3;
    } else if (f32_lt_quiet((float32_t){a}, (float32_t){b})) {
        relation = 0;
    } else {
        relation = 2;
    }
    se_float_shim_finish_relation(out_flags, out_precision_inexact);
    return relation;
}

uint64_t se_float_shim_add_f64(uint64_t a, uint64_t b, uint8_t rounding,
                               uint8_t *out_flags,
                               uint8_t *out_precision_inexact) {
    float64_t result;

    if (!se_float_shim_begin(rounding, out_flags, out_precision_inexact))
        return 0;
    result = f64_add((float64_t){a}, (float64_t){b});
    se_float_shim_finish_f64(out_flags, out_precision_inexact);
    return result.v;
}

uint64_t se_float_shim_sub_f64(uint64_t a, uint64_t b, uint8_t rounding,
                               uint8_t *out_flags,
                               uint8_t *out_precision_inexact) {
    float64_t result;

    if (!se_float_shim_begin(rounding, out_flags, out_precision_inexact))
        return 0;
    result = f64_sub((float64_t){a}, (float64_t){b});
    se_float_shim_finish_f64(out_flags, out_precision_inexact);
    return result.v;
}

uint64_t se_float_shim_mul_f64(uint64_t a, uint64_t b, uint8_t rounding,
                               uint8_t *out_flags,
                               uint8_t *out_precision_inexact) {
    float64_t result;

    if (!se_float_shim_begin(rounding, out_flags, out_precision_inexact))
        return 0;
    result = f64_mul((float64_t){a}, (float64_t){b});
    se_float_shim_finish_f64(out_flags, out_precision_inexact);
    return result.v;
}

uint64_t se_float_shim_div_f64(uint64_t a, uint64_t b, uint8_t rounding,
                               uint8_t *out_flags,
                               uint8_t *out_precision_inexact) {
    float64_t result;

    if (!se_float_shim_begin(rounding, out_flags, out_precision_inexact))
        return 0;
    result = f64_div((float64_t){a}, (float64_t){b});
    se_float_shim_finish_f64(out_flags, out_precision_inexact);
    return result.v;
}

uint64_t se_float_shim_sqrt_f64(uint64_t value, uint8_t rounding,
                                uint8_t *out_flags,
                                uint8_t *out_precision_inexact) {
    float64_t result;

    if (!se_float_shim_begin(rounding, out_flags, out_precision_inexact))
        return 0;
    result = f64_sqrt((float64_t){value});
    se_float_shim_finish_f64(out_flags, out_precision_inexact);
    return result.v;
}

uint8_t se_float_shim_compare_f64(uint64_t a, uint64_t b,
                                  uint8_t *out_flags,
                                  uint8_t *out_precision_inexact) {
    uint8_t relation;

    if (!se_float_shim_begin(0, out_flags, out_precision_inexact))
        return 0;
    if (f64_eq((float64_t){a}, (float64_t){b})) {
        relation = 1;
    } else if (se_float_shim_is_nan_f64(a) ||
               se_float_shim_is_nan_f64(b)) {
        relation = 3;
    } else if (f64_lt_quiet((float64_t){a}, (float64_t){b})) {
        relation = 0;
    } else {
        relation = 2;
    }
    se_float_shim_finish_relation(out_flags, out_precision_inexact);
    return relation;
}

uint64_t se_float_shim_f32_to_f64(uint32_t value, uint8_t *out_flags,
                                  uint8_t *out_precision_inexact) {
    float64_t result;

    if (!se_float_shim_begin(0, out_flags, out_precision_inexact))
        return 0;
    result = f32_to_f64((float32_t){value});
    se_float_shim_finish_f64(out_flags, out_precision_inexact);
    return result.v;
}

uint32_t se_float_shim_f64_to_f32(uint64_t value, uint8_t rounding,
                                  uint8_t *out_flags,
                                  uint8_t *out_precision_inexact) {
    float32_t result;

    if (!se_float_shim_begin(rounding, out_flags, out_precision_inexact))
        return 0;
    result = f64_to_f32((float64_t){value});
    se_float_shim_finish_f32(out_flags, out_precision_inexact);
    return result.v;
}

uint32_t se_float_shim_i32_to_f32(int32_t value, uint8_t rounding,
                                  uint8_t *out_flags,
                                  uint8_t *out_precision_inexact) {
    float32_t result;

    if (!se_float_shim_begin(rounding, out_flags, out_precision_inexact))
        return 0;
    result = i32_to_f32(value);
    se_float_shim_finish_f32(out_flags, out_precision_inexact);
    return result.v;
}

uint32_t se_float_shim_i64_to_f32(int64_t value, uint8_t rounding,
                                  uint8_t *out_flags,
                                  uint8_t *out_precision_inexact) {
    float32_t result;

    if (!se_float_shim_begin(rounding, out_flags, out_precision_inexact))
        return 0;
    result = i64_to_f32(value);
    se_float_shim_finish_f32(out_flags, out_precision_inexact);
    return result.v;
}

uint64_t se_float_shim_i32_to_f64(int32_t value, uint8_t *out_flags,
                                  uint8_t *out_precision_inexact) {
    float64_t result;

    if (!se_float_shim_begin(0, out_flags, out_precision_inexact))
        return 0;
    result = i32_to_f64(value);
    se_float_shim_finish_f64(out_flags, out_precision_inexact);
    return result.v;
}

uint64_t se_float_shim_i64_to_f64(int64_t value, uint8_t rounding,
                                  uint8_t *out_flags,
                                  uint8_t *out_precision_inexact) {
    float64_t result;

    if (!se_float_shim_begin(rounding, out_flags, out_precision_inexact))
        return 0;
    result = i64_to_f64(value);
    se_float_shim_finish_f64(out_flags, out_precision_inexact);
    return result.v;
}

int32_t se_float_shim_f32_to_i32(uint32_t value, uint8_t rounding,
                                 uint8_t *out_flags,
                                 uint8_t *out_precision_inexact) {
    int_fast32_t result;

    if (!se_float_shim_begin(rounding, out_flags, out_precision_inexact))
        return 0;
    result = f32_to_i32((float32_t){value}, softfloat_roundingMode, true);
    se_float_shim_finish_scalar(out_flags, out_precision_inexact);
    return (int32_t)result;
}

int64_t se_float_shim_f32_to_i64(uint32_t value, uint8_t rounding,
                                 uint8_t *out_flags,
                                 uint8_t *out_precision_inexact) {
    int_fast64_t result;

    if (!se_float_shim_begin(rounding, out_flags, out_precision_inexact))
        return 0;
    result = f32_to_i64((float32_t){value}, softfloat_roundingMode, true);
    se_float_shim_finish_scalar(out_flags, out_precision_inexact);
    return (int64_t)result;
}

int32_t se_float_shim_f64_to_i32(uint64_t value, uint8_t rounding,
                                 uint8_t *out_flags,
                                 uint8_t *out_precision_inexact) {
    int_fast32_t result;

    if (!se_float_shim_begin(rounding, out_flags, out_precision_inexact))
        return 0;
    result = f64_to_i32((float64_t){value}, softfloat_roundingMode, true);
    se_float_shim_finish_scalar(out_flags, out_precision_inexact);
    return (int32_t)result;
}

int64_t se_float_shim_f64_to_i64(uint64_t value, uint8_t rounding,
                                 uint8_t *out_flags,
                                 uint8_t *out_precision_inexact) {
    int_fast64_t result;

    if (!se_float_shim_begin(rounding, out_flags, out_precision_inexact))
        return 0;
    result = f64_to_i64((float64_t){value}, softfloat_roundingMode, true);
    se_float_shim_finish_scalar(out_flags, out_precision_inexact);
    return (int64_t)result;
}
