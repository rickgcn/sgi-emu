//! Implements deterministic MIPS architectural transitions and their machine-time shell.
//!
//! Decoding classifies a raw 32-bit word and normalizes supported instructions
//! into typed operands. Execution reads immutable architectural pre-state and
//! produces either a bounded CPU write-set or a synchronous exception request.
//! Normal retirement applies a write-set to general-purpose register and
//! program-counter state. Exception entry instead updates the required `CP0`
//! exception state and redirects the program counter precisely. A scalarized
//! processor-clock model schedules one complete architectural transition per
//! PClk, while timed context methods couple physical bus access to the shared
//! machine timeline.
//!
//! The memory execution surface consists of timed instruction fetch plus `LW` and
//! `SW` through 32-bit-compatible kernel direct segments. TLB-managed translation,
//! privilege-mode validation, guest interrupt acceptance, and `CP1` instructions
//! are outside this surface. Machine time, physical bus topology, event dispatch,
//! and runtime host control remain machine-owned.

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
mod memory;
mod pc;
mod run;
#[cfg(test)]
mod timed_execution_tests;
mod timing;
