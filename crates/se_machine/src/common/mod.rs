//! Shared machine-profile building blocks.
//!
//! Common definitions live here when they are independent of a specific SGI
//! board. Machine profiles may use these definitions while still owning their
//! own topology, event types, and timing ABI.

pub mod timing;
