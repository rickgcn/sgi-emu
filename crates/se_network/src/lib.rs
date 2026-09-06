//! Host-only IPv4 NAT using a private libslirp session and bounded frame queues.
//!
//! No host socket, timer, or queue is part of deterministic machine state.
//! Replay must not construct a network session.

pub mod config;
mod ffi;
mod queue;
pub mod session;
mod worker;
