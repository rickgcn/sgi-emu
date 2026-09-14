//! Shared fixed-capacity byte-range storage contracts.

use std::fmt;
use std::io;

/// The access granted to a storage medium.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageAccess {
    /// The medium is read without writes.
    ReadOnly,
    /// The medium may be read and written.
    ReadWrite,
}

impl fmt::Display for StorageAccess {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadOnly => formatter.write_str("read-only"),
            Self::ReadWrite => formatter.write_str("read-write"),
        }
    }
}

/// A fixed-size storage medium that supports exact byte-range I/O.
pub trait StorageMedium: Send {
    /// Returns the storage capacity in bytes.
    fn size_bytes(&self) -> u64;

    /// Reads exactly one byte range at `offset`.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the complete range cannot be read.
    fn read_exact_at(&mut self, offset: u64, buffer: &mut [u8]) -> io::Result<()>;

    /// Writes exactly one byte range at `offset` without changing capacity.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the complete range cannot be written.
    fn write_all_at(&mut self, offset: u64, data: &[u8]) -> io::Result<()>;
}
