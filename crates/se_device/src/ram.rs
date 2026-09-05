//! Byte-addressed random-access memory.

use se_core::bus::{BusError, DeviceAddr};
use serde::{Deserialize, Serialize};

/// A fixed-size volatile byte store.
#[derive(Clone, Deserialize, Serialize)]
pub struct Ram {
    bytes: Box<[u8]>,
}

impl Ram {
    /// Creates zero-initialized storage with `byte_len` bytes.
    #[must_use]
    pub fn new(byte_len: usize) -> Self {
        Self {
            bytes: vec![0; byte_len].into_boxed_slice(),
        }
    }

    /// Returns the storage capacity in bytes.
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.bytes.len()
    }

    /// Reads one nonempty contiguous byte range in ascending address order.
    ///
    /// # Errors
    ///
    /// Returns [`BusError::InvalidTransaction`] for an empty or overflowing
    /// range, or [`BusError::HardwareFault`] when the complete range is outside
    /// the storage.
    pub fn read(&self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusError> {
        let range = self.transaction_range(address, data.len())?;
        data.copy_from_slice(&self.bytes[range]);
        Ok(())
    }

    /// Writes one nonempty contiguous byte range in ascending address order.
    ///
    /// # Errors
    ///
    /// Returns [`BusError::InvalidTransaction`] for an empty or overflowing
    /// range, or [`BusError::HardwareFault`] when the complete range is outside
    /// the storage. Failures do not change storage.
    pub fn write(&mut self, address: DeviceAddr, data: &[u8]) -> Result<(), BusError> {
        let range = self.transaction_range(address, data.len())?;
        self.bytes[range].copy_from_slice(data);
        Ok(())
    }

    fn transaction_range(
        &self,
        address: DeviceAddr,
        length: usize,
    ) -> Result<std::ops::Range<usize>, BusError> {
        if length == 0 {
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

#[cfg(test)]
mod tests {
    use se_core::bus::{BusError, DeviceAddr};

    use super::Ram;

    #[test]
    fn new_storage_is_zero_initialized() {
        let ram = Ram::new(8);
        let mut bytes = [0xff; 4];

        assert_eq!(ram.byte_len(), 8);
        assert_eq!(ram.read(DeviceAddr::new(4), &mut bytes), Ok(()));
        assert_eq!(bytes, [0; 4]);
    }

    #[test]
    fn transactions_preserve_address_order_for_arbitrary_nonempty_ranges() {
        let mut ram = Ram::new(16);
        let source = [0x10, 0x21, 0x32, 0x43, 0x54, 0x65, 0x76, 0x87];
        let mut destination = [0; 8];

        assert_eq!(ram.write(DeviceAddr::new(3), &source), Ok(()));
        assert_eq!(ram.read(DeviceAddr::new(3), &mut destination), Ok(()));
        assert_eq!(destination, source);
    }

    #[test]
    fn transactions_can_touch_both_storage_boundaries() {
        let mut ram = Ram::new(8);

        assert_eq!(ram.write(DeviceAddr::new(0), &[1, 2, 3, 4]), Ok(()));
        assert_eq!(ram.write(DeviceAddr::new(7), &[8]), Ok(()));

        let mut first = [0; 4];
        let mut last = [0];
        assert_eq!(ram.read(DeviceAddr::new(0), &mut first), Ok(()));
        assert_eq!(ram.read(DeviceAddr::new(7), &mut last), Ok(()));
        assert_eq!(first, [1, 2, 3, 4]);
        assert_eq!(last, [8]);
    }

    #[test]
    fn empty_transactions_are_rejected() {
        let mut ram = Ram::new(8);

        assert_eq!(
            ram.read(DeviceAddr::new(0), &mut []),
            Err(BusError::InvalidTransaction)
        );
        assert_eq!(
            ram.write(DeviceAddr::new(0), &[]),
            Err(BusError::InvalidTransaction)
        );
    }

    #[test]
    fn out_of_bounds_writes_fail_atomically() {
        let mut ram = Ram::new(8);
        ram.write(DeviceAddr::new(4), &[1, 2, 3, 4]).unwrap();

        let mut destination = [0xaa; 2];
        assert_eq!(
            ram.read(DeviceAddr::new(7), &mut destination),
            Err(BusError::HardwareFault)
        );
        assert_eq!(destination, [0xaa; 2]);

        assert_eq!(
            ram.write(DeviceAddr::new(7), &[9, 9]),
            Err(BusError::HardwareFault)
        );
        assert_eq!(
            ram.write(DeviceAddr::new(u64::MAX), &[9]),
            Err(BusError::InvalidTransaction)
        );

        let mut bytes = [0; 4];
        ram.read(DeviceAddr::new(4), &mut bytes).unwrap();
        assert_eq!(bytes, [1, 2, 3, 4]);
    }
}
