/*============================================================================

This C source file contains derived portions of the SoftFloat IEEE
Floating-Point Arithmetic Package, Release 3e, by John R. Hauser.

Copyright 2011, 2012, 2013, 2014, 2015, 2017 The Regents of the University of
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

#include <stdbool.h>
#include <stdint.h>
#include "platform.h"
#include "internals.h"
#include "round_pack.h"

static THREAD_LOCAL uint8_t se_float_round_pack_f32_entered;
static THREAD_LOCAL uint8_t se_float_round_pack_f32_precision_inexact;
static THREAD_LOCAL uint8_t se_float_round_pack_f64_entered;
static THREAD_LOCAL uint8_t se_float_round_pack_f64_precision_inexact;

/*
 * The included algorithm is byte-for-byte upstream source at the fixed
 * revision. Only its externally visible definition name is changed here.
 * Source: source/s_roundPackToF32.c
 * Revision: a0c6494cdc11865811dec815d5c0049fba9d82a8
 */
#undef softfloat_roundPackToF32
#define softfloat_roundPackToF32 se_float_sf_roundPackToF32_impl
#if defined(SE_FLOAT_COMPILER_MSVC)
#pragma warning(push)
#pragma warning(disable : 4102 4244)
#else
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wunused-label"
#endif
#include "s_roundPackToF32.c"
#if defined(SE_FLOAT_COMPILER_MSVC)
#pragma warning(pop)
#else
#pragma GCC diagnostic pop
#endif
#undef softfloat_roundPackToF32

/*
 * The included algorithm is byte-for-byte upstream source at the fixed
 * revision. Only its externally visible definition name is changed here.
 * Source: source/s_roundPackToF64.c
 * Revision: a0c6494cdc11865811dec815d5c0049fba9d82a8
 */
#undef softfloat_roundPackToF64
#define softfloat_roundPackToF64 se_float_sf_roundPackToF64_impl
#if defined(SE_FLOAT_COMPILER_MSVC)
#pragma warning(push)
#pragma warning(disable : 4102 4244)
#else
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wunused-label"
#endif
#include "s_roundPackToF64.c"
#if defined(SE_FLOAT_COMPILER_MSVC)
#pragma warning(pop)
#else
#pragma GCC diagnostic pop
#endif
#undef softfloat_roundPackToF64

float32_t se_float_sf_roundPackToF32(
    bool sign,
    int_fast16_t exp,
    uint_fast32_t sig) {
    se_float_round_pack_f32_entered = 1;
    se_float_round_pack_f32_precision_inexact = (sig & 0x7F) != 0;
    return se_float_sf_roundPackToF32_impl(sign, exp, sig);
}

float64_t se_float_sf_roundPackToF64(
    bool sign,
    int_fast16_t exp,
    uint_fast64_t sig) {
    se_float_round_pack_f64_entered = 1;
    se_float_round_pack_f64_precision_inexact = (sig & 0x3FF) != 0;
    return se_float_sf_roundPackToF64_impl(sign, exp, sig);
}

void se_float_sf_roundPackReset(void) {
    se_float_round_pack_f32_entered = 0;
    se_float_round_pack_f32_precision_inexact = 0;
    se_float_round_pack_f64_entered = 0;
    se_float_round_pack_f64_precision_inexact = 0;
}

uint8_t se_float_sf_roundPackToF32PrecisionInexact(void) {
    return se_float_round_pack_f32_entered
               ? se_float_round_pack_f32_precision_inexact
               : 2;
}

uint8_t se_float_sf_roundPackToF64PrecisionInexact(void) {
    return se_float_round_pack_f64_entered
               ? se_float_round_pack_f64_precision_inexact
               : 2;
}
