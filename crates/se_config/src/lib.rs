//! Data types for describing and editing machine configurations.
//!
//! A definition projects a draft into a fresh view after each edit. Drafts may
//! contain invalid selections; definitions report those problems as diagnostics.

pub mod definition;
pub mod diagnostic;
pub mod draft;
pub mod id;
pub mod value;
pub mod view;
