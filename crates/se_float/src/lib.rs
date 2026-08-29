//! Floating-point formats and interchangeable arithmetic backends.

#![deny(missing_docs)]

/// Floating-point backend selection and operations.
pub mod backend;
/// Guest floating-point bit-pattern types.
pub mod format;
/// Rounding, comparison, exception, and result types.
pub mod operation;

mod native;
mod softfloat;
