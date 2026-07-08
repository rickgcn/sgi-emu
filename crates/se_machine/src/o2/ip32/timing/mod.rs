//! Timing ABI for the SGI O2 IP32 machine profile.
//!
//! The IP32 profile fixes `SimTime` to a 1 GHz opaque tick domain. Device
//! clocks are modeled as ratios or accumulators against this machine timing ABI
//! without changing scheduler semantics.

use crate::common::timing::MachineTiming;

/// Number of simulated ticks per IP32 machine second.
pub const IP32_TIMEBASE_HZ: u64 = 1_000_000_000;

/// Fixed simulated-time ABI for the IP32 machine profile.
pub const IP32_TIMING: MachineTiming = MachineTiming::new(IP32_TIMEBASE_HZ);

#[cfg(test)]
mod tests;
