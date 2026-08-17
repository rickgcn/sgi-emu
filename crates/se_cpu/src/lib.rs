//! Implements deterministic MIPS register, control-flow, and synchronous exception transitions.
//!
//! Decoding classifies a raw 32-bit word and normalizes supported instructions
//! into typed operands. Execution reads immutable architectural pre-state and
//! produces either a bounded CPU write-set or a synchronous exception request.
//! Normal retirement applies a write-set to general-purpose register and
//! program-counter state. Exception entry instead updates the required `CP0`
//! exception state and redirects the program counter precisely.
//!
//! The current semantic kernel does not yet implement machine-timed execution,
//! CPU-originated memory transactions and address translation, `CP1`/floating-point
//! state, snapshot state, or host-control integration. Later milestones add those
//! CPU responsibilities without moving machine-time ownership, physical `Bus`
//! topology, or runtime host control into this crate.

#![forbid(unsafe_op_in_unsafe_fn)]
#![deny(missing_docs)]
// Non-test builds have no external entry point into the private semantic kernel.
#![cfg_attr(not(test), allow(dead_code))]

mod commit;
mod cp0;
mod cpu;
mod decode;
mod exception;
mod execute;
mod gpr;
#[cfg(test)]
mod harness;
mod pc;
