#ifndef SE_FLOAT_ROUND_PACK_H
#define SE_FLOAT_ROUND_PACK_H

#include <stdint.h>

/*
 * Resets the private round-pack observation for the current host thread.
 * The state is separate from SoftFloat flags and has no guest-visible ABI.
 */
void se_float_sf_roundPackReset(void);

/*
 * Returns 0 or 1 for the last binary32 round-pack input, or 2 when no
 * binary32 round-pack call has occurred since the last reset.
 */
uint8_t se_float_sf_roundPackToF32PrecisionInexact(void);

/*
 * Returns 0 or 1 for the last binary64 round-pack input, or 2 when no
 * binary64 round-pack call has occurred since the last reset.
 */
uint8_t se_float_sf_roundPackToF64PrecisionInexact(void);

#endif
