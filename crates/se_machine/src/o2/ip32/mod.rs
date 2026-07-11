//! SGI O2 IP32 machine profile.
//!
//! The IP32 profile defines stable board-level identities, timing, CPU
//! physical address classification, interrupt mapping, and machine-level event
//! orchestration for the SGI O2 workstation.

pub mod address_map;
pub mod bus;
pub mod component_ids;
pub mod event;
pub mod machine;
pub mod timing;
