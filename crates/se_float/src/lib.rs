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
/// binary32 and binary64 results. Standard quiet NaN operands do not
/// themselves report invalid, while signaling NaNs do. Conversion of any NaN
/// to a signed integer reports invalid and returns `None`. Guest NaN polarity,
/// payload selection, status-register updates, traps, and denormal policy
/// remain the caller's responsibility.
#[derive(Debug, Default)]
pub struct SoftFloatBackend;

/// Identifies the host-native backend whose value operations use Rust floats.
///
/// Semantics come from ordinary `f32` and `f64` primitives in the fixed Rust
/// 1.95.0 toolchain. Floating-point results use roundTiesToEven and return the
/// primitive's raw `to_bits` result. NaN results promise only the NaN category,
/// not a stable sign, payload, or encoding. Floating-point-to-signed-integer
/// conversions return `None` when no integer value exists. Native operations
/// do not report IEEE exception flags or [`env::RoundingFacts`].
///
/// External C, C++, Qt, and plugin code must restore the default floating-point
/// control state required by Rust before returning to Rust. This backend does
/// not detect or repair a violation of that integration contract. Its
/// availability does not make it a substitute for [`SoftFloatBackend`] when
/// deterministic values, selectable rounding, flags, or facts are required.
#[derive(Debug, Default)]
pub struct NativeBackend;
