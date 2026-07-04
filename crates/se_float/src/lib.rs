//! Raw-bit IEEE-754 floating-point backends.
//!
//! This crate provides reusable floating-point arithmetic backends for device
//! models. Inputs and outputs are represented as raw IEEE-754 bit patterns so
//! callers can preserve NaN payloads, signed zero, infinities, and subnormal
//! encodings without going through host floating-point values.
//!
//! The crate does not model CPU coprocessor registers, instruction decoding,
//! architectural trap routing, sticky status registers, or machine timing.
//! Architecture-specific layers decide how backend results and exception flags
//! map into their own control/status state.

pub mod backend;
pub mod control;
pub mod result;
pub mod value;
