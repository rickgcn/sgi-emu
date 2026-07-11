//! R5000-compatible MIPS IV processor profile.
//!
//! This module contains R5000 identity, boot-mode stream parsing, configurable
//! profile data, processor execution policy, and the functional CPU component.
//! Board bus routing and machine assembly remain outside the processor model.

pub mod boot_mode;
pub mod cpu;
pub mod execution_policy;
pub mod profile;
pub mod revision;
