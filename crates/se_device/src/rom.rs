//! Canonical read-only byte storage.

use se_core::bus::{BusError, DeviceAddr};

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
    /// Returns [`BusError::InvalidTransaction`] for an invalid length or
    /// address overflow, or [`BusError::HardwareFault`] when the complete
    /// transaction is not backed by this ROM.
    pub fn read(&self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusError> {
        let range = self.transaction_range(address, data.len())?;
        copy_read_transaction(&self.bytes[range], data);
        Ok(())
    }

    /// Rejects one fixed-width write transaction.
    ///
    /// # Errors
    ///
    /// Returns [`BusError::InvalidTransaction`] for an invalid length or
    /// address overflow, [`BusError::HardwareFault`] outside the storage, or
    /// [`BusError::UnimplementedAccess`] for writes to backed storage. The
    /// containing machine determines the hardware write-completion policy.
    pub fn write(&self, address: DeviceAddr, data: &[u8]) -> Result<(), BusError> {
        self.transaction_range(address, data.len())?;
        Err(BusError::UnimplementedAccess)
    }

    fn transaction_range(
        &self,
        address: DeviceAddr,
        length: usize,
    ) -> Result<std::ops::Range<usize>, BusError> {
        if !(1..=4).contains(&length) {
            return Err(BusError::InvalidTransaction);
        }

        let start = address.get();
        let length = u64::try_from(length).map_err(|_| BusError::InvalidTransaction)?;
        let end = start
            .checked_add(length)
            .ok_or(BusError::InvalidTransaction)?;
        if end > self.bytes.len() as u64 {
            return Err(BusError::HardwareFault);
        }

        let start = usize::try_from(start).expect("a backed storage offset fits in usize");
        let end = usize::try_from(end).expect("a backed storage endpoint fits in usize");
        Ok(start..end)
    }
}

fn copy_read_transaction(source: &[u8], data: &mut [u8]) {
    match data {
        [byte0] => {
            *byte0 = source[0];
        }
        [byte0, byte1] => {
            *byte0 = source[0];
            *byte1 = source[1];
        }
        [byte0, byte1, byte2] => {
            *byte0 = source[0];
            *byte1 = source[1];
            *byte2 = source[2];
        }
        [byte0, byte1, byte2, byte3] => {
            *byte0 = source[0];
            *byte1 = source[1];
            *byte2 = source[2];
            *byte3 = source[3];
        }
        _ => unreachable!("ROM transactions contain one through four bytes"),
    }
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusError, DeviceAddr};

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
            Err(BusError::InvalidTransaction)
        );
        assert_eq!(
            rom.read(DeviceAddr::new(0), &mut [0; 5]),
            Err(BusError::InvalidTransaction)
        );
        assert_eq!(
            rom.write(DeviceAddr::new(0), &[]),
            Err(BusError::InvalidTransaction)
        );
        assert_eq!(
            rom.write(DeviceAddr::new(0), &[0; 5]),
            Err(BusError::InvalidTransaction)
        );
    }

    #[test]
    fn rejects_out_of_range_and_crossing_transactions() {
        let rom = Rom::new(vec![0; 4]);
        let mut outside = [0xaa];
        let mut crossing = [0xbb; 3];

        assert_eq!(
            rom.read(DeviceAddr::new(4), &mut outside),
            Err(BusError::HardwareFault)
        );
        assert_eq!(
            rom.read(DeviceAddr::new(2), &mut crossing),
            Err(BusError::HardwareFault)
        );
        assert_eq!(outside, [0xaa]);
        assert_eq!(crossing, [0xbb; 3]);
        assert_eq!(
            rom.read(DeviceAddr::new(u64::MAX), &mut [0; 1]),
            Err(BusError::InvalidTransaction)
        );
        assert_eq!(
            rom.write(DeviceAddr::new(4), &[0]),
            Err(BusError::HardwareFault)
        );
    }

    #[test]
    fn rejects_writes_without_changing_contents() {
        let rom = Rom::new(vec![0x12, 0x34, 0x56, 0x78]);

        assert_eq!(
            rom.write(DeviceAddr::new(1), &[0xaa, 0xbb]),
            Err(BusError::UnimplementedAccess)
        );

        let mut data = [0; 4];
        assert_eq!(rom.read(DeviceAddr::new(0), &mut data), Ok(()));
        assert_eq!(data, [0x12, 0x34, 0x56, 0x78]);
    }
}
