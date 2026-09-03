//! Host-backed storage ports consumed by device models.

use std::io;

/// A fixed-size storage object that supports exact byte-range I/O.
pub trait BlockStorage: Send {
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
