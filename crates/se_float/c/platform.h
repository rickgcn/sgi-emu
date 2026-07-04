#ifndef SE_FLOAT_SOFTFLOAT_PLATFORM_H
#define SE_FLOAT_SOFTFLOAT_PLATFORM_H 1

#ifdef SE_FLOAT_TARGET_LITTLE_ENDIAN
#define LITTLEENDIAN 1
#endif

#ifdef __GNUC_STDC_INLINE__
#define INLINE inline
#else
#define INLINE extern inline
#endif

#include "opts-GCC.h"

#endif
