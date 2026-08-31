//! Silicon Graphics INT2 initial local-interrupt register front end.

use se_core::bus::{BusFault, DeviceAddr};

const LOCAL_INTERRUPT_0_STATUS: u64 = 0x00;
const LOCAL_INTERRUPT_0_MASK: u64 = 0x04;
const LOCAL_INTERRUPT_1_STATUS: u64 = 0x08;
const LOCAL_INTERRUPT_1_MASK: u64 = 0x0c;
const REGISTER_BYTES: u64 = 4;

/// The software-visible INT2 state needed by the IP12 reset path.
pub struct Int2 {
    local_interrupt_masks: [u8; 2],
}

impl Int2 {
    /// Creates an INT2 in its reset state.
    #[must_use]
    #[allow(
        clippy::new_without_default,
        reason = "device construction is intentionally explicit"
    )]
    pub const fn new() -> Self {
        Self {
            local_interrupt_masks: [0; 2],
        }
    }

    /// Restores the mutable INT2 reset state.
    pub fn reset(&mut self) {
        self.local_interrupt_masks = [0; 2];
    }

    /// Reads one fixed-width device-local transaction.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault`] when the complete transaction is not mapped or the
    /// requested width is unsupported.
    pub fn read(&self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusFault> {
        let (register, offset) = decode_register(address, data.len())?;
        let value = match register {
            Register::LocalInterrupt0Status | Register::LocalInterrupt1Status => 0,
            Register::LocalInterrupt0Mask => self.local_interrupt_masks[0],
            Register::LocalInterrupt1Mask => self.local_interrupt_masks[1],
        };
        let bytes = u32::from(value).to_be_bytes();
        data.copy_from_slice(&bytes[offset..offset + data.len()]);
        Ok(())
    }

    /// Writes one fixed-width device-local transaction.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault`] when the complete transaction is not mapped or the
    /// requested width or direction is unsupported.
    pub fn write(&mut self, address: DeviceAddr, data: &[u8]) -> Result<(), BusFault> {
        let (register, offset) = decode_register(address, data.len())?;
        let mask = match register {
            Register::LocalInterrupt0Status | Register::LocalInterrupt1Status => {
                return Err(BusFault::UnsupportedAccess);
            }
            Register::LocalInterrupt0Mask => &mut self.local_interrupt_masks[0],
            Register::LocalInterrupt1Mask => &mut self.local_interrupt_masks[1],
        };

        let mut bytes = u32::from(*mask).to_be_bytes();
        bytes[offset..offset + data.len()].copy_from_slice(data);
        *mask = bytes[3];
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Register {
    LocalInterrupt0Status,
    LocalInterrupt0Mask,
    LocalInterrupt1Status,
    LocalInterrupt1Mask,
}

fn decode_register(address: DeviceAddr, length: usize) -> Result<(Register, usize), BusFault> {
    if !(1..=4).contains(&length) {
        return Err(BusFault::UnsupportedAccess);
    }

    let start = address.get();
    let length = u64::try_from(length).map_err(|_| BusFault::UnsupportedAccess)?;
    let end = start.checked_add(length).ok_or(BusFault::Unmapped)?;

    for (base, register) in [
        (LOCAL_INTERRUPT_0_STATUS, Register::LocalInterrupt0Status),
        (LOCAL_INTERRUPT_0_MASK, Register::LocalInterrupt0Mask),
        (LOCAL_INTERRUPT_1_STATUS, Register::LocalInterrupt1Status),
        (LOCAL_INTERRUPT_1_MASK, Register::LocalInterrupt1Mask),
    ] {
        if start >= base && end <= base + REGISTER_BYTES {
            let offset = usize::try_from(start - base).map_err(|_| BusFault::Unmapped)?;
            return Ok((register, offset));
        }
    }

    Err(BusFault::Unmapped)
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusFault, DeviceAddr};

    use super::{
        Int2, LOCAL_INTERRUPT_0_MASK, LOCAL_INTERRUPT_0_STATUS, LOCAL_INTERRUPT_1_MASK,
        LOCAL_INTERRUPT_1_STATUS,
    };

    fn read_word(int2: &Int2, address: u64) -> Result<u32, BusFault> {
        let mut bytes = [0; 4];
        int2.read(DeviceAddr::new(address), &mut bytes)?;
        Ok(u32::from_be_bytes(bytes))
    }

    #[test]
    fn reset_status_and_masks_are_zero() {
        let int2 = Int2::new();

        for address in [
            LOCAL_INTERRUPT_0_STATUS,
            LOCAL_INTERRUPT_0_MASK,
            LOCAL_INTERRUPT_1_STATUS,
            LOCAL_INTERRUPT_1_MASK,
        ] {
            assert_eq!(read_word(&int2, address), Ok(0));
        }
    }

    #[test]
    fn masks_use_the_low_big_endian_lane() {
        let mut int2 = Int2::new();

        assert_eq!(
            int2.write(DeviceAddr::new(LOCAL_INTERRUPT_0_MASK + 3), &[0xa5]),
            Ok(())
        );
        assert_eq!(
            int2.write(DeviceAddr::new(LOCAL_INTERRUPT_1_MASK), &[0xff]),
            Ok(())
        );
        assert_eq!(read_word(&int2, LOCAL_INTERRUPT_0_MASK), Ok(0xa5));
        assert_eq!(read_word(&int2, LOCAL_INTERRUPT_1_MASK), Ok(0));
    }

    #[test]
    fn status_registers_are_read_only() {
        let mut int2 = Int2::new();

        for address in [LOCAL_INTERRUPT_0_STATUS, LOCAL_INTERRUPT_1_STATUS] {
            assert_eq!(
                int2.write(DeviceAddr::new(address), &[0; 4]),
                Err(BusFault::UnsupportedAccess)
            );
            assert_eq!(read_word(&int2, address), Ok(0));
        }
    }

    #[test]
    fn reset_clears_both_masks() {
        let mut int2 = Int2::new();
        int2.write(DeviceAddr::new(LOCAL_INTERRUPT_0_MASK + 3), &[0x11])
            .unwrap();
        int2.write(DeviceAddr::new(LOCAL_INTERRUPT_1_MASK + 3), &[0x22])
            .unwrap();

        int2.reset();

        assert_eq!(read_word(&int2, LOCAL_INTERRUPT_0_MASK), Ok(0));
        assert_eq!(read_word(&int2, LOCAL_INTERRUPT_1_MASK), Ok(0));
    }

    #[test]
    fn rejects_invalid_unmapped_and_crossing_transactions_atomically() {
        let mut int2 = Int2::new();
        int2.write(DeviceAddr::new(LOCAL_INTERRUPT_0_MASK + 3), &[0x5a])
            .unwrap();

        assert_eq!(
            int2.write(DeviceAddr::new(LOCAL_INTERRUPT_0_MASK), &[]),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(
            int2.write(DeviceAddr::new(LOCAL_INTERRUPT_0_MASK + 3), &[1, 2]),
            Err(BusFault::Unmapped)
        );
        assert_eq!(
            int2.read(DeviceAddr::new(0x10), &mut [0; 1]),
            Err(BusFault::Unmapped)
        );
        assert_eq!(read_word(&int2, LOCAL_INTERRUPT_0_MASK), Ok(0x5a));
    }
}
