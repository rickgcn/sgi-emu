//! Deterministic core primitives for SGI machine emulation.

#![forbid(unsafe_op_in_unsafe_fn)]
#![deny(missing_docs)]

pub mod address;
pub mod bus;
pub mod decode;
pub mod device;
pub mod event;
pub mod inspect;
pub mod interrupt;
pub mod machine;
pub mod save;
pub mod snapshot;
pub mod time;
