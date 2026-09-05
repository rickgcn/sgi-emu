//! Write-only attenuation latches for the IP12 headphone output.

use se_core::bus::{BusError, DeviceAddr};
use serde::{Deserialize, Serialize};

const LEFT_ATTENUATION: u64 = 0;
const RIGHT_ATTENUATION: u64 = 4;

/// The two software-visible MDAC attenuation latches.
#[derive(Clone, Deserialize, Serialize)]
pub struct Mdac {
    attenuation: [u8; 2],
}

impl Mdac {
    /// Creates an MDAC front end in its reset state.
    #[must_use]
    #[allow(
        clippy::new_without_default,
        reason = "device construction is intentionally explicit"
    )]
    pub const fn new() -> Self {
        Self {
            attenuation: [0; 2],
        }
    }

    /// Clears both attenuation latches.
    pub fn reset(&mut self) {
        self.attenuation = [0; 2];
    }

    /// Rejects reads from the write-only latches.
    ///
    /// # Errors
    ///
    /// Returns [`BusError::InvalidTransaction`] for an invalid length.
    /// Other reads return [`BusError::UnimplementedAccess`]; latch readback
    /// is not modeled.
    pub fn read(&self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusError> {
        decode_latch(address, data.len())?;
        Err(BusError::UnimplementedAccess)
    }

    /// Writes one attenuation latch.
    ///
    /// # Errors
    ///
    /// Returns [`BusError`] when the address or width does not select exactly
    /// one latch.
    pub fn write(&mut self, address: DeviceAddr, data: &[u8]) -> Result<(), BusError> {
        let latch = decode_latch(address, data.len())?;
        self.attenuation[latch] = data[0];
        Ok(())
    }
}

fn decode_latch(address: DeviceAddr, length: usize) -> Result<usize, BusError> {
    if !(1..=4).contains(&length) {
        return Err(BusError::InvalidTransaction);
    }

    if length != 1 {
        return Err(BusError::UnimplementedAccess);
    }

    match address.get() {
        LEFT_ATTENUATION => Ok(0),
        RIGHT_ATTENUATION => Ok(1),
        _ => Err(BusError::UnimplementedAccess),
    }
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusError, DeviceAddr};

    use super::{LEFT_ATTENUATION, Mdac, RIGHT_ATTENUATION};

    #[test]
    fn latches_store_independent_byte_values() {
        let mut mdac = Mdac::new();

        assert_eq!(
            mdac.write(DeviceAddr::new(LEFT_ATTENUATION), &[0xa5]),
            Ok(())
        );
        assert_eq!(
            mdac.write(DeviceAddr::new(RIGHT_ATTENUATION), &[0x5a]),
            Ok(())
        );
        assert_eq!(mdac.attenuation, [0xa5, 0x5a]);
    }

    #[test]
    fn reset_clears_both_latches() {
        let mut mdac = Mdac::new();
        mdac.attenuation = [0xa5, 0x5a];

        mdac.reset();

        assert_eq!(mdac.attenuation, [0, 0]);
    }

    #[test]
    fn reads_and_non_byte_accesses_are_rejected() {
        let mut mdac = Mdac::new();

        assert_eq!(
            mdac.read(DeviceAddr::new(LEFT_ATTENUATION), &mut [0]),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(
            mdac.write(DeviceAddr::new(LEFT_ATTENUATION), &[0, 1]),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(
            mdac.write(DeviceAddr::new(2), &[0]),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(mdac.attenuation, [0, 0]);
    }
}
