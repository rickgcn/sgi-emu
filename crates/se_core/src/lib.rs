//! Deterministic core primitives for SGI machine emulation.

#![forbid(unsafe_op_in_unsafe_fn)]
#![deny(missing_docs)]

pub mod address;
pub mod inspect;
pub mod interrupt;
pub mod save;
pub mod time;
