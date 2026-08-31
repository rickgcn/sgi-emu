//! Silicon Graphics HPC1.5 endian-control register front end.

use se_core::bus::{BusFault, DeviceAddr};

const ENDIAN_CONTROL: u64 = 0x00c0;
const REGISTER_BYTES: u64 = 4;
const REVISION: u8 = 0x40;
const WRITABLE_ENDIAN_BITS: u8 = 0x1f;

/// The software-visible HPC1.5 state needed by the IP12 reset path.
pub struct Hpc1 {
    endian_control: u8,
}

impl Hpc1 {
    /// Creates an HPC1.5 in its reset state.
    #[must_use]
    #[allow(
        clippy::new_without_default,
        reason = "device construction is intentionally explicit"
    )]
    pub const fn new() -> Self {
        Self {
            endian_control: REVISION,
        }
    }

    /// Restores the mutable HPC1.5 reset state.
    pub fn reset(&mut self) {
        self.endian_control = REVISION;
    }

    /// Reads one fixed-width device-local transaction.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault`] when the complete transaction is not mapped or the
    /// requested width is unsupported.
    pub fn read(&self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusFault> {
        let offset = register_offset(address, data.len())?;
        let bytes = u32::from(self.endian_control).to_be_bytes();
        data.copy_from_slice(&bytes[offset..offset + data.len()]);
        Ok(())
    }

    /// Writes one fixed-width device-local transaction.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault`] when the complete transaction is not mapped or the
    /// requested width is unsupported.
    pub fn write(&mut self, address: DeviceAddr, data: &[u8]) -> Result<(), BusFault> {
        let offset = register_offset(address, data.len())?;
        let mut bytes = u32::from(self.endian_control).to_be_bytes();
        bytes[offset..offset + data.len()].copy_from_slice(data);
        self.endian_control = REVISION | (bytes[3] & WRITABLE_ENDIAN_BITS);
        Ok(())
    }
}

fn register_offset(address: DeviceAddr, length: usize) -> Result<usize, BusFault> {
    if !(1..=4).contains(&length) {
        return Err(BusFault::UnsupportedAccess);
    }

    let start = address.get();
    let length = u64::try_from(length).map_err(|_| BusFault::UnsupportedAccess)?;
    let end = start.checked_add(length).ok_or(BusFault::Unmapped)?;
    if start < ENDIAN_CONTROL || end > ENDIAN_CONTROL + REGISTER_BYTES {
        return Err(BusFault::Unmapped);
    }

    usize::try_from(start - ENDIAN_CONTROL).map_err(|_| BusFault::Unmapped)
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusFault, DeviceAddr};

    use super::{ENDIAN_CONTROL, Hpc1};

    fn read_word(hpc1: &Hpc1) -> Result<u32, BusFault> {
        let mut bytes = [0; 4];
        hpc1.read(DeviceAddr::new(ENDIAN_CONTROL), &mut bytes)?;
        Ok(u32::from_be_bytes(bytes))
    }

    #[test]
    fn reset_value_reports_hpc1_5_revision() {
        let hpc1 = Hpc1::new();

        assert_eq!(read_word(&hpc1), Ok(0x40));
    }

    #[test]
    fn writable_bits_use_the_low_big_endian_lane() {
        let mut hpc1 = Hpc1::new();

        assert_eq!(
            hpc1.write(DeviceAddr::new(ENDIAN_CONTROL + 3), &[0x1f]),
            Ok(())
        );
        assert_eq!(read_word(&hpc1), Ok(0x5f));

        let mut high_lane = [0xff];
        assert_eq!(
            hpc1.read(DeviceAddr::new(ENDIAN_CONTROL), &mut high_lane),
            Ok(())
        );
        assert_eq!(high_lane, [0]);
    }

    #[test]
    fn writes_preserve_revision_and_clear_reserved_bit() {
        let mut hpc1 = Hpc1::new();

        assert_eq!(
            hpc1.write(DeviceAddr::new(ENDIAN_CONTROL), &[0xff; 4]),
            Ok(())
        );
        assert_eq!(read_word(&hpc1), Ok(0x5f));
        assert_eq!(hpc1.write(DeviceAddr::new(ENDIAN_CONTROL), &[0; 4]), Ok(()));
        assert_eq!(read_word(&hpc1), Ok(0x40));
    }

    #[test]
    fn reset_clears_writable_endian_controls() {
        let mut hpc1 = Hpc1::new();
        hpc1.write(DeviceAddr::new(ENDIAN_CONTROL + 3), &[0x1f])
            .unwrap();

        hpc1.reset();

        assert_eq!(read_word(&hpc1), Ok(0x40));
    }

    #[test]
    fn rejects_invalid_unmapped_and_crossing_transactions_atomically() {
        let mut hpc1 = Hpc1::new();
        hpc1.write(DeviceAddr::new(ENDIAN_CONTROL + 3), &[0x12])
            .unwrap();

        assert_eq!(
            hpc1.write(DeviceAddr::new(ENDIAN_CONTROL), &[]),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(
            hpc1.write(DeviceAddr::new(ENDIAN_CONTROL + 3), &[0xaa, 0xbb]),
            Err(BusFault::Unmapped)
        );
        assert_eq!(
            hpc1.read(DeviceAddr::new(0), &mut [0; 1]),
            Err(BusFault::Unmapped)
        );
        assert_eq!(read_word(&hpc1), Ok(0x52));
    }
}
