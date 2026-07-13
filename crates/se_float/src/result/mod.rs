//! Floating-point operation result containers.

use crate::control::FloatExceptionFlags;

/// Value returned by a floating-point backend operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct FloatResult<T> {
    /// Operation value.
    pub value: T,

    /// IEEE exception flags raised while producing `value`.
    pub flags: FloatExceptionFlags,
}

impl<T> FloatResult<T> {
    /// Creates a floating-point result.
    pub const fn new(value: T, flags: FloatExceptionFlags) -> Self {
        Self { value, flags }
    }
}

#[cfg(test)]
mod tests;
