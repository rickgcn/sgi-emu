//! Functional MIPS IV instruction execution.
//!
//! The execution target composes the architecture helpers into precise
//! instruction boundaries and external CPU bus transactions. Processor-specific
//! reset, exception-vector, and cache-coherence policy is supplied separately.

pub mod access;
pub mod block;
pub mod bus;
mod cp0;
mod fpu;
mod integer;
mod memory;
pub mod policy;
pub mod port;
pub mod state;
pub mod target;

#[cfg(test)]
mod tests;
