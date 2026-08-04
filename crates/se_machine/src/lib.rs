//! Machine profiles and mutable integrations for supported systems.
//!
//! A machine profile defines hardware configuration, board topology, component
//! identity, address maps, protocol wiring, event semantics, and machine state.
//! Each integration delegates its event loop to the machine-independent
//! facilities in `se_runtime`.

pub mod common;
pub mod indigo;
pub mod o2;
