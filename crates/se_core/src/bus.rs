//! Physical bus address and transaction contracts.

/// A byte address in the machine's physical address space.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysAddr(u64);

impl PhysAddr {
    /// Creates a physical address from its numeric value.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the numeric value of the physical address.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// An error raised by a physical bus transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BusFault {
    /// The addressed physical location is not mapped.
    Unmapped,

    /// The requested transaction is not supported by the target.
    UnsupportedAccess,
}

/// An interface for fixed-width physical bus transactions.
///
/// Buffer elements correspond to consecutive physical addresses in ascending
/// order. Transaction widths are expressed by buffer length. The contract
/// recognizes lengths of one, two, and four bytes; implementations return
/// [`BusFault::UnsupportedAccess`] for other lengths or unsupported accesses.
pub trait PhysicalBus {
    /// Reads one transaction into `data`.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault`] when the address or transaction is not supported.
    fn read(&mut self, address: PhysAddr, data: &mut [u8]) -> Result<(), BusFault>;

    /// Writes one transaction from `data`.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault`] when the address or transaction is not supported.
    fn write(&mut self, address: PhysAddr, data: &[u8]) -> Result<(), BusFault>;
}

#[cfg(test)]
mod tests {
    use super::PhysAddr;

    #[test]
    fn physical_address_round_trips() {
        let address = PhysAddr::new(0x1fc0_0000);

        assert_eq!(address.get(), 0x1fc0_0000);
    }
}
