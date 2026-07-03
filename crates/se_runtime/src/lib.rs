//! Runtime orchestration for the emulator.
//!
//! This crate connects core primitives into an event-driven execution layer. It
//! owns components, advances the simulated-time scheduler, and records
//! structured trace facts.
//!
//! The runtime uses [`se_core::scheduler::SimTime`] as its only time input.
//! Host wall-clock time, sleeping, and real-time pacing belong to outer
//! integration layers, not to runtime semantics.

pub mod registry;
pub mod runtime;
