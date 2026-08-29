// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef SE_FLOAT_SOFTFLOAT_PLATFORM_H
#define SE_FLOAT_SOFTFLOAT_PLATFORM_H

#define LITTLEENDIAN 1

#if defined(_MSC_VER)
#define INLINE __inline
#define THREAD_LOCAL __declspec(thread)
#else
#define INLINE inline
#define THREAD_LOCAL _Thread_local
#endif

#endif
