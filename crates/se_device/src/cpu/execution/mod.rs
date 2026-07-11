//! ISA-independent functional CPU execution.
//!
//! This module defines a request/completion state machine for functional CPU
//! models. Instruction semantics, architectural state, bus protocol payloads,
//! and timing policy are supplied by an execution target.

pub mod functional;
pub mod protocol;
pub mod target;

#[cfg(test)]
mod tests;
