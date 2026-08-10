//! Defines the private boundary for host-native value operations.
//!
//! Host-native operations use Rust floating-point primitives under the
//! toolchain's default floating-point environment. They do not share an
//! interface with deterministic operations that report flags and facts.
