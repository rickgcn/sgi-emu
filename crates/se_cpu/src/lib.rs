//! Implements deterministic MIPS architectural transitions and their machine-time shell.
//!
//! Decoding classifies a raw 32-bit word and normalizes supported instructions
//! into typed operands. Execution reads immutable architectural pre-state and
//! produces either a bounded CPU write-set or a synchronous exception request.
//! Instruction commits apply bounded general-purpose register, `CP0`, and
//! program-counter effects. Exception entry instead updates the required `CP0`
//! exception state and redirects the program counter precisely. A scalarized
//! processor-clock model schedules one complete architectural transition per
//! PClk, while timed context methods couple physical bus access to the shared
//! machine timeline.
//!
//! The memory execution surface consists of timed instruction fetch plus `LW` and
//! `SW` through canonical 32-bit virtual-address classification. Kernel direct
//! segments and the `Status.ERL` low-address route bypass a CPU-local 64-entry TLB
//! with fixed 4 KiB pages; mapped segments use its VPN2, ASID, global, validity,
//! and dirty semantics. Variable pages, extended 64-bit address spaces, and `CP1`
//! instructions are outside this surface.
//! External R10000 interrupt inputs are sampled at architectural boundaries and
//! use the same exception-entry path as synchronous exceptions. Machine time,
//! physical bus topology, event dispatch, and runtime host control remain
//! machine-owned.

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
mod interrupt;
mod memory;
mod pc;
mod run;
#[cfg(test)]
mod timed_execution_tests;
mod timing;
mod tlb;
