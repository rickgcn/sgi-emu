//! MIPS I architecture building blocks.
//!
//! This module contains reusable MIPS I architecture state and helpers. It does
//! not define a concrete processor package, board integration, bus timing, or
//! machine-specific reset wiring.

pub mod config;
pub mod exception;
pub mod gpr;
pub mod instruction;
