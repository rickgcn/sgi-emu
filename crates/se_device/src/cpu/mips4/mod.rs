//! MIPS IV architecture building blocks.
//!
//! This module contains reusable MIPS IV architecture state and helpers. It does
//! not define a concrete processor package, board integration, bus timing, or
//! machine-specific reset wiring.

pub mod alu;
pub mod branch;
pub mod config;
pub mod exception;
pub mod gpr;
pub mod instruction;
