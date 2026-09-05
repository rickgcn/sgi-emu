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
pub enum BusError {
    /// The emulated hardware transaction failed.
    HardwareFault,

    /// The caller supplied an invalid transaction.
    InvalidTransaction,

    /// The requested hardware behavior is not implemented.
    UnimplementedAccess,
}

/// An interface for fixed-width physical bus transactions.
///
/// Buffer elements correspond to consecutive physical addresses in ascending
/// order. Transaction widths are expressed by buffer length. The contract
/// recognizes lengths from one through four bytes, including one atomic
/// three-byte transaction. Invalid lengths and address overflow return
/// [`BusError::InvalidTransaction`] before any target side effects.
///
/// Hardware failures and unimplemented behavior remain distinct. The machine
/// completes hardware failures according to its bus contract; it must not
/// convert invalid or unimplemented requests into hardware error state.
pub trait PhysicalBus {
    /// Reads one transaction into `data`.
    ///
    /// # Errors
    ///
    /// Returns [`BusError::HardwareFault`] for a hardware failure that the
    /// machine has not otherwise completed, [`BusError::InvalidTransaction`]
    /// for an invalid request, or [`BusError::UnimplementedAccess`] for
    /// behavior that the target does not implement.
    fn read(&mut self, address: PhysAddr, data: &mut [u8]) -> Result<(), BusError>;

    /// Writes one transaction from `data`.
    ///
    /// # Errors
    ///
    /// Returns [`BusError::HardwareFault`] for a hardware failure that the
    /// machine has not otherwise completed, [`BusError::InvalidTransaction`]
    /// for an invalid request, or [`BusError::UnimplementedAccess`] for
    /// behavior that the target does not implement.
    fn write(&mut self, address: PhysAddr, data: &[u8]) -> Result<(), BusError>;
}

#[cfg(test)]
mod tests {
    use super::{BusError, DeviceAddr, PhysAddr, PhysicalBus};

    #[derive(Default)]
    struct RecordingBus {
        transactions: Vec<(PhysAddr, usize)>,
    }

    impl PhysicalBus for RecordingBus {
        fn read(&mut self, address: PhysAddr, data: &mut [u8]) -> Result<(), BusError> {
            if !(1..=4).contains(&data.len()) {
                return Err(BusError::InvalidTransaction);
            }

            self.transactions.push((address, data.len()));
            data.fill(0);
            Ok(())
        }

        fn write(&mut self, address: PhysAddr, data: &[u8]) -> Result<(), BusError> {
            if !(1..=4).contains(&data.len()) {
                return Err(BusError::InvalidTransaction);
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
            Err(BusError::InvalidTransaction)
        );
        assert_eq!(
            bus.write(PhysAddr::new(0), &[0; 5]),
            Err(BusError::InvalidTransaction)
        );
        assert!(bus.transactions.is_empty());
    }
}
