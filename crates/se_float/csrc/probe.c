#include <stdint.h>
#include "platform.h"
#include "round_pack.h"
#include "shim.h"

_Static_assert(sizeof(uint32_t) == 4, "se_float requires 32-bit uint32_t");
_Static_assert(sizeof(uint64_t) == 8, "se_float requires 64-bit uint64_t");
_Static_assert(UINT_FAST8_MAX == UINT8_MAX,
               "se_float requires an eight-bit uint_fast8_t");
_Static_assert(sizeof(void *) == 8, "se_float requires 64-bit pointers");

static THREAD_LOCAL uint8_t se_float_probe_tls;
static THREAD_LOCAL uint8_t se_float_probe_tls = 0;

_Static_assert(sizeof(se_float_probe_tls) == 1,
               "se_float TLS declarations must preserve byte width");
