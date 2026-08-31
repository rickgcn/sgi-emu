//! Canonical read-only byte storage.

use se_core::bus::{BusFault, DeviceAddr};

/// An immutable device-local byte array.
pub struct Rom {
    bytes: Box<[u8]>,
}

impl Rom {
    /// Creates a ROM from bytes in ascending device-address order.
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes: bytes.into_boxed_slice(),
        }
    }

    /// Reads one fixed-width transaction.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault::UnsupportedAccess`] unless `data` contains one
    /// through four bytes. Returns [`BusFault::Unmapped`] when the complete
    /// transaction is not backed by this ROM.
    pub fn read(&self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusFault> {
        let range = self.transaction_range(address, data.len())?;
        data.copy_from_slice(&self.bytes[range]);
        Ok(())
    }

    /// Rejects one fixed-width write transaction.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault::UnsupportedAccess`] for a supported-width write to
    /// mapped storage, or [`BusFault::Unmapped`] when the complete transaction
    /// is outside the ROM.
    pub fn write(&self, address: DeviceAddr, data: &[u8]) -> Result<(), BusFault> {
        self.transaction_range(address, data.len())?;
        Err(BusFault::UnsupportedAccess)
    }

    fn transaction_range(
        &self,
        address: DeviceAddr,
        length: usize,
    ) -> Result<std::ops::Range<usize>, BusFault> {
        if !(1..=4).contains(&length) {
            return Err(BusFault::UnsupportedAccess);
        }

        let start = usize::try_from(address.get()).map_err(|_| BusFault::Unmapped)?;
        let end = start.checked_add(length).ok_or(BusFault::Unmapped)?;
        if end > self.bytes.len() {
            return Err(BusFault::Unmapped);
        }

        Ok(start..end)
    }
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusFault, DeviceAddr};

    use super::Rom;

    #[test]
    fn reads_all_supported_transaction_widths_in_address_order() {
        let rom = Rom::new(vec![0x10, 0x21, 0x32, 0x43, 0x54]);

        for length in 1..=4 {
            let mut data = vec![0; length];
            assert_eq!(rom.read(DeviceAddr::new(1), &mut data), Ok(()));
            assert_eq!(data, [0x21, 0x32, 0x43, 0x54][..length]);
        }
    }

    #[test]
    fn rejects_unsupported_transaction_widths() {
        let rom = Rom::new(vec![0; 8]);

        assert_eq!(
            rom.read(DeviceAddr::new(0), &mut []),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(
            rom.read(DeviceAddr::new(0), &mut [0; 5]),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(
            rom.write(DeviceAddr::new(0), &[]),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(
            rom.write(DeviceAddr::new(0), &[0; 5]),
            Err(BusFault::UnsupportedAccess)
        );
    }

    #[test]
    fn rejects_out_of_range_and_crossing_transactions() {
        let rom = Rom::new(vec![0; 4]);

        assert_eq!(
            rom.read(DeviceAddr::new(4), &mut [0; 1]),
            Err(BusFault::Unmapped)
        );
        assert_eq!(
            rom.read(DeviceAddr::new(2), &mut [0; 3]),
            Err(BusFault::Unmapped)
        );
        assert_eq!(
            rom.read(DeviceAddr::new(u64::MAX), &mut [0; 1]),
            Err(BusFault::Unmapped)
        );
        assert_eq!(rom.write(DeviceAddr::new(4), &[0]), Err(BusFault::Unmapped));
    }

    #[test]
    fn rejects_writes_without_changing_contents() {
        let rom = Rom::new(vec![0x12, 0x34, 0x56, 0x78]);

        assert_eq!(
            rom.write(DeviceAddr::new(1), &[0xaa, 0xbb]),
            Err(BusFault::UnsupportedAccess)
        );

        let mut data = [0; 4];
        assert_eq!(rom.read(DeviceAddr::new(0), &mut data), Ok(()));
        assert_eq!(data, [0x12, 0x34, 0x56, 0x78]);
    }
}
