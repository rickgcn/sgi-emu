//! Primitive values stored in editable drafts.

use serde::{Deserialize, Serialize};

/// A property value independent of its editor metadata.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PropertyValue {
    /// A Boolean value.
    Bool(bool),
    /// A signed integer value.
    Integer(i64),
    /// A text value, including values edited as paths.
    Text(String),
}
