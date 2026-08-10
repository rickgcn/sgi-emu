//! Provides ISA-independent IEEE 754 control and result contracts.
//!
//! [`SoftFloatBackend`] identifies the deterministic SoftFloat-backed engine,
//! while [`NativeBackend`] identifies the narrower host-native engine. The
//! backends are concrete, stateless handles with intentionally distinct
//! contracts. IEEE binary32 and binary64 values cross their boundary as raw
//! `u32` and `u64` bit patterns.
//!
//! This crate does not define guest register state, exception traps, NaN
//! policies, denormal flushing, or processor-model behavior. Callers apply
//! those policies around the primitive operation layer.

#![forbid(unsafe_op_in_unsafe_fn)]
#![deny(missing_docs)]

mod bits;
mod native;
mod softfloat;

pub mod env;

/// Identifies the deterministic backend backed by Berkeley SoftFloat.
///
/// Operation state belongs to a complete C-side transaction and thread-local
/// storage rather than to this value. Sharing a reference does not merge
/// rounding modes or exception state between calls. The engine preserves
/// gradual underflow, reports per-operation flags and rounding facts, and uses
/// positive canonical quiet NaNs `0x7FC00000` and `0x7FF8000000000000` for
/// binary32 and binary64 results.
#[derive(Debug, Default)]
pub struct SoftFloatBackend;

/// Identifies the host-native backend whose value operations use Rust floats.
///
/// Native operations use roundTiesToEven and do not report IEEE exception
/// flags or [`env::RoundingFacts`]. Their NaN bit patterns follow the fixed
/// Rust toolchain rather than the canonical SoftFloat result convention. The
/// surrounding process must preserve Rust's default floating-point control
/// environment across external calls.
#[derive(Debug, Default)]
pub struct NativeBackend;
