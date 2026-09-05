//! Motorola DSP56001 CPU-visible external SRAM.

use se_core::bus::{BusError, DeviceAddr};
use serde::{Deserialize, Serialize};

const WORD_COUNT: usize = 32 * 1024;
const WORD_BYTES: u64 = 4;
const BYTE_LEN: u64 = WORD_COUNT as u64 * WORD_BYTES;
const DATA_MASK: u32 = 0x00ff_ffff;

/// The CPU-visible external memory of the IP12 DSP56001 subsystem.
#[derive(Clone, Deserialize, Serialize)]
pub struct Dsp56001 {
    words: Box<[u32]>,
}

impl Dsp56001 {
    /// Creates a DSP subsystem with zero-filled external SRAM.
    #[must_use]
    pub fn new() -> Self {
        Self {
            words: vec![0; WORD_COUNT].into_boxed_slice(),
        }
    }

    /// Reads one 24-bit DSP word through its 32-bit CPU slot.
    ///
    /// # Errors
    ///
    /// Returns [`BusError::InvalidTransaction`] for an invalid length, or
    /// [`BusError::UnimplementedAccess`] unless the transaction is one
    /// aligned four-byte word. Returns [`BusError::HardwareFault`] when the word is
    /// outside the external SRAM window.
    pub fn read(&self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusError> {
        let index = word_index(address, data.len())?;
        data.copy_from_slice(&self.words[index].to_be_bytes());
        Ok(())
    }

    /// Writes the low 24 bits of one CPU word to external SRAM.
    ///
    /// # Errors
    ///
    /// Returns [`BusError::InvalidTransaction`] for an invalid length, or
    /// [`BusError::UnimplementedAccess`] unless the transaction is one
    /// aligned four-byte word. Returns [`BusError::HardwareFault`] without changing
    /// memory when the word is outside the external SRAM window.
    pub fn write(&mut self, address: DeviceAddr, data: &[u8]) -> Result<(), BusError> {
        let index = word_index(address, data.len())?;
        let value = u32::from_be_bytes(data.try_into().map_err(|_| BusError::UnimplementedAccess)?);
        self.words[index] = value & DATA_MASK;
        Ok(())
    }
}

impl Default for Dsp56001 {
    fn default() -> Self {
        Self::new()
    }
}

fn word_index(address: DeviceAddr, length: usize) -> Result<usize, BusError> {
    if !(1..=4).contains(&length) {
        return Err(BusError::InvalidTransaction);
    }

    if length != WORD_BYTES as usize || !address.get().is_multiple_of(WORD_BYTES) {
        return Err(BusError::UnimplementedAccess);
    }
    if address.get() >= BYTE_LEN {
        return Err(BusError::HardwareFault);
    }
    usize::try_from(address.get() / WORD_BYTES).map_err(|_| BusError::HardwareFault)
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusError, DeviceAddr};

    use super::{BYTE_LEN, Dsp56001};

    fn read_word(device: &Dsp56001, address: u64) -> Result<u32, BusError> {
        let mut bytes = [0; 4];
        device.read(DeviceAddr::new(address), &mut bytes)?;
        Ok(u32::from_be_bytes(bytes))
    }

    #[test]
    fn new_memory_is_zero_initialized() {
        let device = Dsp56001::new();

        assert_eq!(read_word(&device, 0), Ok(0));
        assert_eq!(read_word(&device, BYTE_LEN - 4), Ok(0));
    }

    #[test]
    fn writes_store_only_the_low_twenty_four_bits() {
        let mut device = Dsp56001::new();

        assert_eq!(
            device.write(DeviceAddr::new(4), &0xab12_3456_u32.to_be_bytes()),
            Ok(())
        );
        assert_eq!(read_word(&device, 4), Ok(0x0012_3456));
    }

    #[test]
    fn first_and_last_words_are_independent() {
        let mut device = Dsp56001::new();

        device
            .write(DeviceAddr::new(0), &0x0001_0203_u32.to_be_bytes())
            .unwrap();
        device
            .write(
                DeviceAddr::new(BYTE_LEN - 4),
                &0x0004_0506_u32.to_be_bytes(),
            )
            .unwrap();

        assert_eq!(read_word(&device, 0), Ok(0x0001_0203));
        assert_eq!(read_word(&device, BYTE_LEN - 4), Ok(0x0004_0506));
    }

    #[test]
    fn invalid_transactions_do_not_modify_memory() {
        let mut device = Dsp56001::new();
        device
            .write(DeviceAddr::new(0), &0x0012_3456_u32.to_be_bytes())
            .unwrap();

        assert_eq!(
            device.write(DeviceAddr::new(1), &[0; 4]),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(
            device.write(DeviceAddr::new(0), &[0; 2]),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(
            device.write(DeviceAddr::new(BYTE_LEN), &[0; 4]),
            Err(BusError::HardwareFault)
        );
        assert_eq!(read_word(&device, 0), Ok(0x0012_3456));
    }
}
