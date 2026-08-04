//! Machine-independent runtime orchestration for the emulator.
//!
//! This crate owns the generic component registry, simulated-time event loop,
//! runtime bookkeeping, and structured tracing context. Machine crates supply
//! event payloads and dispatch semantics through closures.
//!
//! The runtime uses [`se_core::scheduler::SimTime`] as its only time input.
//! Host wall-clock time, sleeping, and real-time pacing belong to outer
//! integration layers, not to runtime semantics.

pub mod registry;
pub mod runtime;
