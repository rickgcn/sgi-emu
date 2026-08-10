/*============================================================================

This C source file contains derived portions of the SoftFloat IEEE
Floating-Point Arithmetic Package, Release 3e, by John R. Hauser.

Copyright 2011, 2012, 2013, 2014, 2015, 2016 The Regents of the University of
California.  All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

 1. Redistributions of source code must retain the above copyright notice,
    this list of conditions, and the following disclaimer.

 2. Redistributions in binary form must reproduce the above copyright notice,
    this list of conditions, and the following disclaimer in the documentation
    and/or other materials provided with the distribution.

 3. Neither the name of the University nor the names of its contributors may
    be used to endorse or promote products derived from this software without
    specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE REGENTS AND CONTRIBUTORS "AS IS", AND ANY
EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE, ARE
DISCLAIMED.  IN NO EVENT SHALL THE REGENTS OR CONTRIBUTORS BE LIABLE FOR ANY
DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND
ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

=============================================================================*/

#include <stdint.h>
#include "platform.h"
#include "primitives.h"

#if defined(SE_FLOAT_COMPILER_MSVC)
#pragma warning(push)
#pragma warning(disable : 4146 4244)
#endif

/*
 * Derived from source/s_shortShiftRightJam64.c at revision
 * a0c6494cdc11865811dec815d5c0049fba9d82a8. Only the definition name and
 * shared include organization differ from upstream.
 */
uint64_t se_float_sf_shortShiftRightJam64(uint64_t a, uint_fast8_t dist) {

    return a >> dist | ((a & (((uint_fast64_t)1 << dist) - 1)) != 0);
}

/*
 * Derived from source/s_shiftRightJam32.c at revision
 * a0c6494cdc11865811dec815d5c0049fba9d82a8. Only the definition name and
 * shared include organization differ from upstream.
 */
uint32_t se_float_sf_shiftRightJam32(uint32_t a, uint_fast16_t dist) {

    return (dist < 31) ? a >> dist | ((uint32_t)(a << (-dist & 31)) != 0)
                       : (a != 0);
}

/*
 * Derived from source/s_shiftRightJam64.c at revision
 * a0c6494cdc11865811dec815d5c0049fba9d82a8. Only the definition name and
 * shared include organization differ from upstream.
 */
uint64_t se_float_sf_shiftRightJam64(uint64_t a, uint_fast32_t dist) {

    return (dist < 63) ? a >> dist | ((uint64_t)(a << (-dist & 63)) != 0)
                       : (a != 0);
}

/*
 * Derived from source/s_shiftRightJam64Extra.c at revision
 * a0c6494cdc11865811dec815d5c0049fba9d82a8. Only the definition name and
 * shared include organization differ from upstream.
 */
struct uint64_extra se_float_sf_shiftRightJam64Extra(
    uint64_t a,
    uint64_t extra,
    uint_fast32_t dist) {
    struct uint64_extra z;

    if (dist < 64) {
        z.v = a >> dist;
        z.extra = a << (-dist & 63);
    } else {
        z.v = 0;
        z.extra = (dist == 64) ? a : (a != 0);
    }
    z.extra |= (extra != 0);
    return z;
}

/*
 * Derived from source/s_countLeadingZeros32.c at revision
 * a0c6494cdc11865811dec815d5c0049fba9d82a8. Only the definition name and
 * shared include organization differ from upstream.
 */
uint_fast8_t se_float_sf_countLeadingZeros32(uint32_t a) {
    uint_fast8_t count;

    count = 0;
    if (a < 0x10000) {
        count = 16;
        a <<= 16;
    }
    if (a < 0x1000000) {
        count += 8;
        a <<= 8;
    }
    count += softfloat_countLeadingZeros8[a >> 24];
    return count;
}

/*
 * Derived from source/s_countLeadingZeros64.c at revision
 * a0c6494cdc11865811dec815d5c0049fba9d82a8. Only the definition name and
 * shared include organization differ from upstream.
 */
uint_fast8_t se_float_sf_countLeadingZeros64(uint64_t a) {
    uint_fast8_t count;
    uint32_t a32;

    count = 0;
    a32 = a >> 32;
    if (!a32) {
        count = 32;
        a32 = a;
    }
    /*------------------------------------------------------------------------
    | From here, result is current count + count leading zeros of `a32'.
    *------------------------------------------------------------------------*/
    if (a32 < 0x10000) {
        count += 16;
        a32 <<= 16;
    }
    if (a32 < 0x1000000) {
        count += 8;
        a32 <<= 8;
    }
    count += softfloat_countLeadingZeros8[a32 >> 24];
    return count;
}

/*
 * Derived from source/s_mul64To128.c at revision
 * a0c6494cdc11865811dec815d5c0049fba9d82a8. Only the definition name and
 * shared include organization differ from upstream.
 */
struct uint128 se_float_sf_mul64To128(uint64_t a, uint64_t b) {
    uint32_t a32, a0, b32, b0;
    struct uint128 z;
    uint64_t mid1, mid;

    a32 = a >> 32;
    a0 = a;
    b32 = b >> 32;
    b0 = b;
    z.v0 = (uint_fast64_t)a0 * b0;
    mid1 = (uint_fast64_t)a32 * b0;
    mid = mid1 + (uint_fast64_t)a0 * b32;
    z.v64 = (uint_fast64_t)a32 * b32;
    z.v64 += (uint_fast64_t)(mid < mid1) << 32 | mid >> 32;
    mid <<= 32;
    z.v0 += mid;
    z.v64 += (z.v0 < mid);
    return z;
}

/*
 * Derived from source/s_approxRecip32_1.c at revision
 * a0c6494cdc11865811dec815d5c0049fba9d82a8. Only the definition name and
 * shared include organization differ from upstream.
 */
uint32_t se_float_sf_approxRecip32_1(uint32_t a) {
    int index;
    uint16_t eps, r0;
    uint32_t sigma0;
    uint_fast32_t r;
    uint32_t sqrSigma0;

    index = a >> 27 & 0xF;
    eps = (uint16_t)(a >> 11);
    r0 = softfloat_approxRecip_1k0s[index] -
         ((softfloat_approxRecip_1k1s[index] * (uint_fast32_t)eps) >> 20);
    sigma0 = ~(uint_fast32_t)((r0 * (uint_fast64_t)a) >> 7);
    r = ((uint_fast32_t)r0 << 16) + ((r0 * (uint_fast64_t)sigma0) >> 24);
    sqrSigma0 = ((uint_fast64_t)sigma0 * sigma0) >> 32;
    r += ((uint32_t)r * (uint_fast64_t)sqrSigma0) >> 48;
    return r;
}

/*
 * Derived from source/s_approxRecipSqrt32_1.c at revision
 * a0c6494cdc11865811dec815d5c0049fba9d82a8. Only the definition name and
 * shared include organization differ from upstream.
 */
uint32_t se_float_sf_approxRecipSqrt32_1(unsigned int oddExpA, uint32_t a) {
    int index;
    uint16_t eps, r0;
    uint_fast32_t ESqrR0;
    uint32_t sigma0;
    uint_fast32_t r;
    uint32_t sqrSigma0;

    index = (a >> 27 & 0xE) + oddExpA;
    eps = (uint16_t)(a >> 12);
    r0 = softfloat_approxRecipSqrt_1k0s[index] -
         ((softfloat_approxRecipSqrt_1k1s[index] * (uint_fast32_t)eps) >> 20);
    ESqrR0 = (uint_fast32_t)r0 * r0;
    if (!oddExpA)
        ESqrR0 <<= 1;
    sigma0 = ~(uint_fast32_t)(((uint32_t)ESqrR0 * (uint_fast64_t)a) >> 23);
    r = ((uint_fast32_t)r0 << 16) + ((r0 * (uint_fast64_t)sigma0) >> 25);
    sqrSigma0 = ((uint_fast64_t)sigma0 * sigma0) >> 32;
    r += ((uint32_t)((r >> 1) + (r >> 3) - ((uint_fast32_t)r0 << 14)) *
          (uint_fast64_t)sqrSigma0) >>
         48;
    if (!(r & 0x80000000))
        r = 0x80000000;
    return r;
}

#if defined(SE_FLOAT_COMPILER_MSVC)
#pragma warning(pop)
#endif
