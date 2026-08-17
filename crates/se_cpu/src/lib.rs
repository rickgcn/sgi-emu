//! Implements deterministic MIPS register and control-flow transitions.
//!
//! Decoding classifies a raw 32-bit word and normalizes supported instructions
//! into typed operands. Execution reads immutable architectural pre-state and
//! produces a bounded CPU write-set. Normal retirement applies that write-set to
//! general-purpose register and program-counter state.
//!
//! Machine time, memory transactions, address translation, coprocessor state,
//! snapshots, and host control are outside this crate's responsibility.

#![forbid(unsafe_op_in_unsafe_fn)]
#![deny(missing_docs)]
// Non-test builds have no external entry point into the private semantic kernel.
#![cfg_attr(not(test), allow(dead_code))]

mod commit;
mod cpu;
mod decode;
mod exception;
mod execute;
mod gpr;
#[cfg(test)]
mod harness;
mod pc;
