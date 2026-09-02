//! Host-backed block storage used by concrete machine compositions.

use std::io;

/// A fixed-size storage object that supports exact reads at byte offsets.
pub trait BlockStorage: Send {
    /// Returns the storage capacity in bytes.
    fn size_bytes(&self) -> u64;

    /// Reads exactly one byte range at `offset`.
    ///
    /// # Errors
    ///
    /// Returns the host I/O error when the complete range cannot be read.
    fn read_exact_at(&mut self, offset: u64, buffer: &mut [u8]) -> io::Result<()>;
}
