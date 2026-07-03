//! Machine profiles and board-level integration.
//!
//! This crate contains machine-specific profiles built on top of `se_core` and
//! `se_runtime`. A machine profile fixes board topology, component identity, and
//! the physical meaning of internal simulated time for that machine family.
//!
//! Chip behavior, address decoding, bus transactions, firmware images, and
//! host-time pacing are represented by dedicated machine integration modules.

pub mod common;
pub mod indigo;
