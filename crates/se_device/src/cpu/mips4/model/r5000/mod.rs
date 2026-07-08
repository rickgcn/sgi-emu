//! R5000-compatible MIPS IV processor profile.
//!
//! This module contains R5000 identity and configurable profile data. Board
//! policy, boot-mode wiring, CP0 Config reset construction, cache operation
//! execution, and instruction execution are modeled by later integration layers.

pub mod profile;
pub mod revision;
