//! Defines deterministic, machine-independent contracts for SGI emulation.
//!
//! [`time`] and [`event`] define the machine timeline, while [`interrupt`] exposes
//! per-CPU guest delivery, host wake, and burst truncation. [`address`], [`bus`],
//! and [`decode`] translate CPU and DMA transactions into device-local accesses
//! without coupling device identity to address mappings, and [`device`] joins
//! MMIO, events, state, and introspection for registered devices. [`save`] and
//! [`snapshot`] preserve private component state, and [`inspect`] and [`machine`]
//! provide object-safe runtime surfaces.
//!
//! This crate defines no CPU instruction set, concrete device, machine topology,
//! host run loop, or wall-clock policy. Machine implementations supply those
//! choices while preserving the identities, ordering, and snapshot contracts
//! defined here.

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
