//! Shared fixed-capacity byte-range storage contracts.

use std::io;

/// The access granted to a storage medium.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageAccess {
    /// The medium is read without writes.
    ReadOnly,
    /// The medium may be read and written.
    ReadWrite,
}

/// A fixed-size storage medium that supports exact byte-range I/O.
pub trait StorageMedium: Send {
    /// Returns the storage capacity in bytes.
    fn size_bytes(&self) -> u64;

    /// Reads exactly one byte range at `offset`.
    ///
    /// # Errors
    ///
    /// Returns the host I/O error when the complete range cannot be read.
    fn read_exact_at(&mut self, offset: u64, buffer: &mut [u8]) -> io::Result<()>;

    /// Writes exactly one byte range at `offset` without changing capacity.
    ///
    /// # Errors
    ///
    /// Returns the host I/O error when the complete range cannot be written.
    fn write_all_at(&mut self, offset: u64, data: &[u8]) -> io::Result<()>;
}
