#ifndef SE_FLOAT_PLATFORM_H
#define SE_FLOAT_PLATFORM_H

#if (defined(SE_FLOAT_COMPILER_GCC) + defined(SE_FLOAT_COMPILER_APPLE_CLANG) + \
     defined(SE_FLOAT_COMPILER_MSVC)) != 1
#error "se_float requires exactly one supported compiler profile"
#endif

#if !defined(SOFTFLOAT_FAST_INT64)
#error "se_float requires the SOFTFLOAT_FAST_INT64 profile"
#endif

#if !defined(SE_FLOAT_TARGET_LITTLE_ENDIAN)
#error "se_float supports only the validated little-endian targets"
#endif

#define LITTLEENDIAN 1
#define INLINE inline

#if defined(SE_FLOAT_COMPILER_GCC)
#if !defined(__GNUC__) || defined(__clang__) || !defined(__linux__) || \
    !defined(__x86_64__)
#error "se_float requires GCC for x86_64-unknown-linux-gnu"
#endif
#define THREAD_LOCAL _Thread_local
#elif defined(SE_FLOAT_COMPILER_APPLE_CLANG)
#if !defined(__clang__) || !defined(__apple_build_version__) || \
    !defined(__APPLE__) || !defined(__aarch64__)
#error "se_float requires Apple Clang for aarch64-apple-darwin"
#endif
#define THREAD_LOCAL _Thread_local
#elif defined(SE_FLOAT_COMPILER_MSVC)
#if !defined(_MSC_VER) || !defined(_M_X64) || defined(__clang__)
#error "se_float requires MSVC for x86_64-pc-windows-msvc"
#endif
#define THREAD_LOCAL __declspec(thread)
#endif

#include <stdint.h>
#include "primitiveTypes.h"
#include "rename.h"

uint64_t se_float_sf_shortShiftRightJam64(uint64_t a, uint_fast8_t dist);
uint32_t se_float_sf_shiftRightJam32(uint32_t a, uint_fast16_t dist);
uint64_t se_float_sf_shiftRightJam64(uint64_t a, uint_fast32_t dist);
struct uint64_extra se_float_sf_shiftRightJam64Extra(
    uint64_t a,
    uint64_t extra,
    uint_fast32_t dist);
uint_fast8_t se_float_sf_countLeadingZeros32(uint32_t a);
uint_fast8_t se_float_sf_countLeadingZeros64(uint64_t a);
struct uint128 se_float_sf_mul64To128(uint64_t a, uint64_t b);
uint32_t se_float_sf_approxRecip32_1(uint32_t a);
uint32_t se_float_sf_approxRecipSqrt32_1(unsigned int odd_exp_a, uint32_t a);

#define softfloat_shortShiftRightJam64 se_float_sf_shortShiftRightJam64
#define softfloat_shiftRightJam32 se_float_sf_shiftRightJam32
#define softfloat_shiftRightJam64 se_float_sf_shiftRightJam64
#define softfloat_shiftRightJam64Extra se_float_sf_shiftRightJam64Extra
#define softfloat_countLeadingZeros32 se_float_sf_countLeadingZeros32
#define softfloat_countLeadingZeros64 se_float_sf_countLeadingZeros64
#define softfloat_mul64To128 se_float_sf_mul64To128
#define softfloat_approxRecip32_1 se_float_sf_approxRecip32_1
#define softfloat_approxRecipSqrt32_1 se_float_sf_approxRecipSqrt32_1

#endif
