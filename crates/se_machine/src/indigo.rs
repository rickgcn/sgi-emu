//! SGI Indigo machine models.

use serde::{Deserialize, Serialize};

pub mod ip12;

/// A graphics board available when constructing an Indigo.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum GraphicsBoard {
    /// The entry graphics board built from REX1, VC1, and Bt479.
    Lg1,
}
