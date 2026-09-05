//! Write-only attenuation latches for the IP12 headphone output.

use se_core::bus::{BusFault, DeviceAddr};
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
    /// Returns [`BusFault::UnsupportedAccess`] for a decoded byte latch and
    /// [`BusFault::Unmapped`] for other addresses.
    pub fn read(&self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusFault> {
        decode_latch(address, data.len())?;
        Err(BusFault::UnsupportedAccess)
    }

    /// Writes one attenuation latch.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault`] when the address or width does not select exactly
    /// one latch.
    pub fn write(&mut self, address: DeviceAddr, data: &[u8]) -> Result<(), BusFault> {
        let latch = decode_latch(address, data.len())?;
        self.attenuation[latch] = data[0];
        Ok(())
    }
}

fn decode_latch(address: DeviceAddr, length: usize) -> Result<usize, BusFault> {
    if length != 1 {
        return Err(BusFault::UnsupportedAccess);
    }

    match address.get() {
        LEFT_ATTENUATION => Ok(0),
        RIGHT_ATTENUATION => Ok(1),
        _ => Err(BusFault::Unmapped),
    }
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusFault, DeviceAddr};

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
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(
            mdac.write(DeviceAddr::new(LEFT_ATTENUATION), &[0, 1]),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(
            mdac.write(DeviceAddr::new(2), &[0]),
            Err(BusFault::Unmapped)
        );
        assert_eq!(mdac.attenuation, [0, 0]);
    }
}
