#ifndef SE_FLOAT_SHIM_H
#define SE_FLOAT_SHIM_H

#include <stdint.h>

/*
 * Every entry point accepts and returns only fixed-width integers. Binary32
 * and binary64 values are raw IEEE bit patterns. Rounding tags are 0/1/2/3
 * for nearest-even, toward-zero, toward-positive, and toward-negative.
 * Relation tags are 0/1/2/3 for less, equal, greater, and unordered.
 *
 * Each output pointer must be non-null and point to one writable uint8_t.
 * Flags use bits 0 through 4 for inexact, underflow, overflow, division by
 * zero, and invalid. Precision-inexact is 0 or 1. An invalid rounding tag or
 * internal observation failure writes flag byte 0x80 and fact byte 2; callers
 * must reject every output from that transaction.
 *
 * A call sets the rounding mode, fixes after-rounding tininess, clears flags
 * and observations, executes the complete operation, and copies results
 * before returning. All mutable C state is thread-local, and valid calls do
 * not inherit state from an earlier call on the same thread.
 */

uint32_t se_float_shim_add_f32(uint32_t a, uint32_t b, uint8_t rounding,
                               uint8_t *out_flags,
                               uint8_t *out_precision_inexact);
uint32_t se_float_shim_sub_f32(uint32_t a, uint32_t b, uint8_t rounding,
                               uint8_t *out_flags,
                               uint8_t *out_precision_inexact);
uint32_t se_float_shim_mul_f32(uint32_t a, uint32_t b, uint8_t rounding,
                               uint8_t *out_flags,
                               uint8_t *out_precision_inexact);
uint32_t se_float_shim_div_f32(uint32_t a, uint32_t b, uint8_t rounding,
                               uint8_t *out_flags,
                               uint8_t *out_precision_inexact);
uint32_t se_float_shim_sqrt_f32(uint32_t value, uint8_t rounding,
                                uint8_t *out_flags,
                                uint8_t *out_precision_inexact);
uint8_t se_float_shim_compare_f32(uint32_t a, uint32_t b,
                                  uint8_t *out_flags,
                                  uint8_t *out_precision_inexact);

uint64_t se_float_shim_add_f64(uint64_t a, uint64_t b, uint8_t rounding,
                               uint8_t *out_flags,
                               uint8_t *out_precision_inexact);
uint64_t se_float_shim_sub_f64(uint64_t a, uint64_t b, uint8_t rounding,
                               uint8_t *out_flags,
                               uint8_t *out_precision_inexact);
uint64_t se_float_shim_mul_f64(uint64_t a, uint64_t b, uint8_t rounding,
                               uint8_t *out_flags,
                               uint8_t *out_precision_inexact);
uint64_t se_float_shim_div_f64(uint64_t a, uint64_t b, uint8_t rounding,
                               uint8_t *out_flags,
                               uint8_t *out_precision_inexact);
uint64_t se_float_shim_sqrt_f64(uint64_t value, uint8_t rounding,
                                uint8_t *out_flags,
                                uint8_t *out_precision_inexact);
uint8_t se_float_shim_compare_f64(uint64_t a, uint64_t b,
                                  uint8_t *out_flags,
                                  uint8_t *out_precision_inexact);

uint64_t se_float_shim_f32_to_f64(uint32_t value, uint8_t *out_flags,
                                  uint8_t *out_precision_inexact);
uint32_t se_float_shim_f64_to_f32(uint64_t value, uint8_t rounding,
                                  uint8_t *out_flags,
                                  uint8_t *out_precision_inexact);
uint32_t se_float_shim_i32_to_f32(int32_t value, uint8_t rounding,
                                  uint8_t *out_flags,
                                  uint8_t *out_precision_inexact);
uint32_t se_float_shim_i64_to_f32(int64_t value, uint8_t rounding,
                                  uint8_t *out_flags,
                                  uint8_t *out_precision_inexact);
uint64_t se_float_shim_i32_to_f64(int32_t value, uint8_t *out_flags,
                                  uint8_t *out_precision_inexact);
uint64_t se_float_shim_i64_to_f64(int64_t value, uint8_t rounding,
                                  uint8_t *out_flags,
                                  uint8_t *out_precision_inexact);
int32_t se_float_shim_f32_to_i32(uint32_t value, uint8_t rounding,
                                 uint8_t *out_flags,
                                 uint8_t *out_precision_inexact);
int64_t se_float_shim_f32_to_i64(uint32_t value, uint8_t rounding,
                                 uint8_t *out_flags,
                                 uint8_t *out_precision_inexact);
int32_t se_float_shim_f64_to_i32(uint64_t value, uint8_t rounding,
                                 uint8_t *out_flags,
                                 uint8_t *out_precision_inexact);
int64_t se_float_shim_f64_to_i64(uint64_t value, uint8_t rounding,
                                 uint8_t *out_flags,
                                 uint8_t *out_precision_inexact);

#endif
