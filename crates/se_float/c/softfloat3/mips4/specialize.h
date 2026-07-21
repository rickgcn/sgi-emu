/*============================================================================

This C header file is part of the SoftFloat IEEE Floating-Point Arithmetic
Package, Release 3e, by John R. Hauser.

Copyright 2011, 2012, 2013, 2014, 2015, 2016, 2018 The Regents of the
University of California.  All rights reserved.

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

#ifndef se_float_mips4_specialize_h
#define se_float_mips4_specialize_h 1

#include <stdbool.h>
#include <stdint.h>
#include "primitiveTypes.h"
#include "softfloat.h"

#define init_detectTininess softfloat_tininess_afterRounding

#define i32_fromPosOverflow  0x7FFFFFFF
#define i32_fromNegOverflow  (-0x7FFFFFFF - 1)
#define i32_fromNaN          0x7FFFFFFF

#define i64_fromPosOverflow  INT64_C( 0x7FFFFFFFFFFFFFFF )
#define i64_fromNegOverflow  (-INT64_C( 0x7FFFFFFFFFFFFFFF ) - 1)
#define i64_fromNaN          INT64_C( 0x7FFFFFFFFFFFFFFF )

struct commonNaN {
    bool sign;
#ifdef LITTLEENDIAN
    uint64_t v0, v64;
#else
    uint64_t v64, v0;
#endif
};

#define defaultNaNF32UI 0x7FBFFFFF
#define softfloat_isSigNaNF32UI( uiA ) (((uiA) & 0x7FC00000) == 0x7FC00000)

void softfloat_f32UIToCommonNaN( uint_fast32_t uiA, struct commonNaN *zPtr );
uint_fast32_t softfloat_commonNaNToF32UI( const struct commonNaN *aPtr );
uint_fast32_t
 softfloat_propagateNaNF32UI( uint_fast32_t uiA, uint_fast32_t uiB );

#define defaultNaNF64UI UINT64_C( 0x7FF7FFFFFFFFFFFF )
#define softfloat_isSigNaNF64UI( uiA ) \
    (((uiA) & UINT64_C( 0x7FF8000000000000 )) == \
     UINT64_C( 0x7FF8000000000000 ))

void softfloat_f64UIToCommonNaN( uint_fast64_t uiA, struct commonNaN *zPtr );
uint_fast64_t softfloat_commonNaNToF64UI( const struct commonNaN *aPtr );
uint_fast64_t
 softfloat_propagateNaNF64UI( uint_fast64_t uiA, uint_fast64_t uiB );

#endif
