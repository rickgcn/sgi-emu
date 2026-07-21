//! MIPS IV specialization of the Berkeley SoftFloat 3e backend.
//!
//! This backend uses the legacy MIPS NaN encoding and the MIPS IV default
//! results. Processor-specific handling of unimplemented operations and flush
//! behavior remains in the processor model.

/// SoftFloat 3e backend specialized for MIPS IV floating-point semantics.
#[derive(Clone, Copy, Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct Mips4SoftFloatBackend;

impl Mips4SoftFloatBackend {
    /// Creates a MIPS IV SoftFloat backend.
    pub const fn new() -> Self {
        Self
    }
}
