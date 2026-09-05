//! SGI Centronics external status and remote-control latch.

use se_core::bus::{BusFault, DeviceAddr};
use serde::{Deserialize, Serialize};

const REGISTER_BYTES: u64 = 4;
const STATUS_LANE: usize = 1;
const STATUS_BITS: u8 = 0x0f;
const REMOTE_BITS: u8 = 0x03;

/// The board-level Centronics state visible through the HPC1 external slot.
#[derive(Clone, Deserialize, Serialize)]
pub struct CentronicsPort {
    status_input: u8,
    remote_output: u8,
}

impl CentronicsPort {
    /// Creates a disconnected port with inactive raw status inputs.
    #[must_use]
    #[allow(
        clippy::new_without_default,
        reason = "device construction is intentionally explicit"
    )]
    pub const fn new() -> Self {
        Self {
            status_input: 0,
            remote_output: 0,
        }
    }

    /// Restores the guest-driven output latch without changing external inputs.
    pub fn reset(&mut self) {
        self.remote_output = 0;
    }

    /// Reads the external status byte through its big-endian GIO lane.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault`] when the transaction is outside the external slot,
    /// crosses its boundary, or has an unsupported width or alignment.
    pub fn read(&self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusFault> {
        let offset = register_transaction(address, data.len())?;
        data.fill(0);
        if transaction_contains_status_lane(offset, data.len()) {
            data[STATUS_LANE - offset] = self.status_input & STATUS_BITS;
        }
        Ok(())
    }

    /// Writes the remote-control byte through its big-endian GIO lane.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault`] when the transaction is outside the external slot,
    /// crosses its boundary, or has an unsupported width or alignment.
    pub fn write(&mut self, address: DeviceAddr, data: &[u8]) -> Result<(), BusFault> {
        let offset = register_transaction(address, data.len())?;
        if transaction_contains_status_lane(offset, data.len()) {
            self.remote_output = data[STATUS_LANE - offset] & REMOTE_BITS;
        }
        Ok(())
    }
}

fn register_transaction(address: DeviceAddr, length: usize) -> Result<usize, BusFault> {
    if !matches!(length, 1 | 2 | 4) {
        return Err(BusFault::UnsupportedAccess);
    }

    let start = address.get();
    if start >= REGISTER_BYTES {
        return Err(BusFault::Unmapped);
    }
    let length = u64::try_from(length).map_err(|_| BusFault::UnsupportedAccess)?;
    let end = start
        .checked_add(length)
        .ok_or(BusFault::UnsupportedAccess)?;
    if end > REGISTER_BYTES || !start.is_multiple_of(length) {
        return Err(BusFault::UnsupportedAccess);
    }

    usize::try_from(start).map_err(|_| BusFault::UnsupportedAccess)
}

const fn transaction_contains_status_lane(offset: usize, length: usize) -> bool {
    offset <= STATUS_LANE && STATUS_LANE < offset + length
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusFault, DeviceAddr};

    use super::CentronicsPort;

    fn read(port: &CentronicsPort, address: u64, length: usize) -> Result<Vec<u8>, BusFault> {
        let mut data = vec![0; length];
        port.read(DeviceAddr::new(address), &mut data)?;
        Ok(data)
    }

    #[test]
    fn disconnected_port_reads_zero_without_reflecting_remote_output() {
        let mut port = CentronicsPort::new();

        assert_eq!(read(&port, 1, 1), Ok(vec![0]));
        port.write(DeviceAddr::new(1), &[3]).unwrap();

        assert_eq!(port.remote_output, 3);
        assert_eq!(read(&port, 1, 1), Ok(vec![0]));
    }

    #[test]
    fn reset_clears_remote_output_and_preserves_external_status() {
        let mut port = CentronicsPort::new();
        port.status_input = 0xff;
        port.write(DeviceAddr::new(1), &[3]).unwrap();

        port.reset();

        assert_eq!(port.remote_output, 0);
        assert_eq!(read(&port, 1, 1), Ok(vec![0x0f]));
    }

    #[test]
    fn byte_halfword_and_word_accesses_select_gio_bits_twenty_three_through_sixteen() {
        let mut port = CentronicsPort::new();
        port.status_input = 0x0a;

        assert_eq!(read(&port, 0, 4), Ok(vec![0, 0x0a, 0, 0]));
        assert_eq!(read(&port, 0, 2), Ok(vec![0, 0x0a]));
        assert_eq!(read(&port, 2, 2), Ok(vec![0, 0]));
        for offset in 0..4 {
            let expected = if offset == 1 { 0x0a } else { 0 };
            assert_eq!(read(&port, offset, 1), Ok(vec![expected]));
        }

        port.write(DeviceAddr::new(0), &0x0003_0000_u32.to_be_bytes())
            .unwrap();
        assert_eq!(port.remote_output, 3);
        port.write(DeviceAddr::new(0), &[0, 1]).unwrap();
        assert_eq!(port.remote_output, 1);
        port.write(DeviceAddr::new(2), &[0xff, 0xff]).unwrap();
        assert_eq!(port.remote_output, 1);
        port.write(DeviceAddr::new(0), &[0xff]).unwrap();
        assert_eq!(port.remote_output, 1);
    }

    #[test]
    fn remote_output_keeps_only_reset_and_port_enable_bits() {
        let mut port = CentronicsPort::new();

        port.write(DeviceAddr::new(1), &[0xff]).unwrap();

        assert_eq!(port.remote_output, 3);
    }

    #[test]
    fn rejects_invalid_widths_alignments_and_slot_crossings_atomically() {
        let mut port = CentronicsPort::new();
        port.write(DeviceAddr::new(1), &[3]).unwrap();

        for (address, length) in [(0, 0), (0, 3), (1, 2), (2, 4), (3, 2)] {
            assert_eq!(
                port.write(DeviceAddr::new(address), &vec![0; length]),
                Err(BusFault::UnsupportedAccess)
            );
        }
        assert_eq!(
            port.write(DeviceAddr::new(4), &[0]),
            Err(BusFault::Unmapped)
        );
        assert_eq!(port.remote_output, 3);
    }
}
