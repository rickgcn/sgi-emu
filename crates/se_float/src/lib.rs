//! Provides ISA-independent IEEE 754 binary32 and binary64 primitives.
//!
//! The always-available [`SoftFloatBackend`] and [`NativeBackend`] are concrete,
//! stateless handles with intentionally different contracts. Floating-point
//! values cross both APIs as raw `u32` binary32 or `u64` binary64 bit patterns.
//! The accurate backend accepts [`env::RoundingMode`] where rounding is needed
//! and returns [`env::Outcome`] values with per-operation non-trapping flags and
//! [`env::RoundingFacts`]. The native backend is fixed to roundTiesToEven and
//! returns values without flags or facts.
//!
//! This crate does not hold guest registers or snapshot state and does not
//! implement MIPS, CP1, FCSR, trap, NaN-mapping, denormal-flushing, or processor-
//! model policy. Callers apply those policies around this primitive layer and
//! explicitly select a backend.

#![forbid(unsafe_op_in_unsafe_fn)]
#![deny(missing_docs)]

mod bits;
mod native;
mod softfloat;

pub mod env;

/// Provides accurate IEEE binary32 and binary64 primitives backed by Berkeley SoftFloat.
///
/// Rounding-sensitive methods accept all four [`env::RoundingMode`] variants:
/// nearest-even, toward zero, toward positive infinity, and toward negative
/// infinity. Every call returns an [`env::Outcome`] containing only that
/// operation's value, non-trapping [`env::ExceptionFlags`], and neutral
/// [`env::RoundingFacts`]. Binary32 and binary64 inputs and results use raw
/// `u32` and `u64` IEEE bit patterns, and gradual underflow is preserved.
///
/// The backend uses standard IEEE signaling/quiet NaN polarity. Floating-point
/// NaN results are positive canonical quiet NaNs: `0x7FC00000` for binary32 and
/// `0x7FF8000000000000` for binary64. A quiet NaN operand does not by itself
/// report invalid, while a signaling NaN does. Floating-point-to-signed-integer
/// conversion returns [`env::Outcome<Option<T>>`](env::Outcome), with `None`
/// exactly when [`env::ExceptionFlags::INVALID`] is present.
///
/// This handle contains no guest state. Each method performs a complete private
/// C transaction, and mutable SoftFloat state is isolated with C11 thread-local
/// storage. MIPS, CP1, FCSR, trap, NaN-mapping, and denormal policies remain
/// outside this backend.
#[derive(Debug, Default)]
pub struct SoftFloatBackend;

/// Provides host-native value primitives using Rust `f32` and `f64` operations.
///
/// Semantics come from ordinary Rust primitives in the fixed Rust 1.95.0
/// toolchain. Operations are fixed to roundTiesToEven, accept no rounding mode,
/// and return values without IEEE exception flags or [`env::RoundingFacts`].
/// Floating-point results are the primitive's raw `to_bits` value. A NaN result
/// promises only the NaN category, not a stable sign, payload, or encoding.
/// Floating-point-to-signed-integer conversion returns `None` only when no
/// integer value exists; it does not report an invalid-operation flag.
///
/// External C, C++, Qt, and plugin code must restore the default floating-point
/// control state required by Rust before returning to Rust. This backend does
/// not read, manage, detect, or repair the host floating-point environment. The
/// handle contains no guest state and is not an accurate-backend substitute
/// when selectable rounding, flags, or facts are required.
#[derive(Debug, Default)]
pub struct NativeBackend;
