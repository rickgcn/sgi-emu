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

/// A byte offset in a device's local address space.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceAddr(u64);

impl DeviceAddr {
    /// Creates a device address from its numeric value.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the numeric value of the device address.
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
/// recognizes lengths from one through four bytes; implementations return
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
    use super::{BusFault, DeviceAddr, PhysAddr, PhysicalBus};

    #[derive(Default)]
    struct RecordingBus {
        transactions: Vec<(PhysAddr, usize)>,
    }

    impl PhysicalBus for RecordingBus {
        fn read(&mut self, address: PhysAddr, data: &mut [u8]) -> Result<(), BusFault> {
            if !(1..=4).contains(&data.len()) {
                return Err(BusFault::UnsupportedAccess);
            }

            self.transactions.push((address, data.len()));
            data.fill(0);
            Ok(())
        }

        fn write(&mut self, address: PhysAddr, data: &[u8]) -> Result<(), BusFault> {
            if !(1..=4).contains(&data.len()) {
                return Err(BusFault::UnsupportedAccess);
            }

            self.transactions.push((address, data.len()));
            Ok(())
        }
    }

    #[test]
    fn physical_address_round_trips() {
        let address = PhysAddr::new(0x1fc0_0000);

        assert_eq!(address.get(), 0x1fc0_0000);
    }

    #[test]
    fn device_address_round_trips() {
        let address = DeviceAddr::new(0x1234);

        assert_eq!(address.get(), 0x1234);
    }

    #[test]
    fn three_byte_access_is_one_transaction() {
        let mut bus = RecordingBus::default();

        bus.write(PhysAddr::new(0x100), &[1, 2, 3])
            .expect("three-byte transaction should be supported");

        assert_eq!(bus.transactions, vec![(PhysAddr::new(0x100), 3)]);
    }

    #[test]
    fn widths_outside_one_through_four_are_rejected() {
        let mut bus = RecordingBus::default();

        assert_eq!(
            bus.write(PhysAddr::new(0), &[]),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(
            bus.write(PhysAddr::new(0), &[0; 5]),
            Err(BusFault::UnsupportedAccess)
        );
        assert!(bus.transactions.is_empty());
    }
}
