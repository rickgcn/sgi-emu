//! Timing ABI for the SGI Indigo IP12 machine profile.
//!
//! The IP12 profile fixes `SimTime` to a 3.3 GHz opaque tick domain. This keeps
//! 30 MHz, 33 MHz, and related board clocks representable by integer ratios in
//! machine timing code without changing scheduler or runtime semantics.

use crate::common::timing::MachineTiming;

/// Number of simulated ticks per IP12 machine second.
pub const IP12_TIMEBASE_HZ: u64 = 3_300_000_000;

/// Fixed simulated-time ABI for the IP12 machine profile.
pub const IP12_TIMING: MachineTiming = MachineTiming::new(IP12_TIMEBASE_HZ);

#[cfg(test)]
mod tests;
