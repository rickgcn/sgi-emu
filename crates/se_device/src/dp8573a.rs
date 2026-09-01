//! National Semiconductor DP8573A register storage front end.

use se_core::bus::{BusFault, DeviceAddr};

const REGISTER_COUNT: usize = 32;
const BANKED_REGISTER_COUNT: usize = 4;
const MAIN_STATUS: usize = 0;
const REGISTER_SELECT: u8 = 1 << 6;

/// The software-visible register storage exercised by the IP12 PROM.
pub struct Dp8573a {
    registers: [u8; REGISTER_COUNT],
    alternate_control_registers: [u8; BANKED_REGISTER_COUNT],
}

impl Dp8573a {
    /// Creates an RTC front end with cleared register storage.
    #[must_use]
    #[allow(
        clippy::new_without_default,
        reason = "device construction is intentionally explicit"
    )]
    pub const fn new() -> Self {
        Self {
            registers: [0; REGISTER_COUNT],
            alternate_control_registers: [0; BANKED_REGISTER_COUNT],
        }
    }

    /// Reads one IP12 RTC transaction.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault::UnsupportedAccess`] unless the transaction is a
    /// byte on the low byte lane or a complete aligned word.
    pub fn read(&self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusFault> {
        let register = decode_register(address, data.len())?;
        let value = self.read_register(register);
        match data.len() {
            1 => data[0] = value,
            4 => data.copy_from_slice(&u32::from(value).to_be_bytes()),
            _ => unreachable!(),
        }
        Ok(())
    }

    /// Writes one IP12 RTC transaction.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault::UnsupportedAccess`] unless the transaction is a
    /// byte on the low byte lane or a complete aligned word.
    pub fn write(&mut self, address: DeviceAddr, data: &[u8]) -> Result<(), BusFault> {
        let register = decode_register(address, data.len())?;
        let value = data[data.len() - 1];
        if (1..=BANKED_REGISTER_COUNT).contains(&register)
            && self.registers[MAIN_STATUS] & REGISTER_SELECT != 0
        {
            self.alternate_control_registers[register - 1] = value;
        } else {
            self.registers[register] = value;
        }
        Ok(())
    }

    fn read_register(&self, register: usize) -> u8 {
        if (1..=BANKED_REGISTER_COUNT).contains(&register)
            && self.registers[MAIN_STATUS] & REGISTER_SELECT != 0
        {
            self.alternate_control_registers[register - 1]
        } else {
            self.registers[register]
        }
    }
}

fn decode_register(address: DeviceAddr, length: usize) -> Result<usize, BusFault> {
    let address = address.get();
    let register = match length {
        1 if address & 3 == 3 => address >> 2,
        4 if address & 3 == 0 => address >> 2,
        _ => return Err(BusFault::UnsupportedAccess),
    };
    usize::try_from(register)
        .ok()
        .filter(|register| *register < REGISTER_COUNT)
        .ok_or(BusFault::UnsupportedAccess)
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusFault, DeviceAddr};

    use super::Dp8573a;

    const HOURS_COMPARE: u64 = 0x57;
    const DAY_OF_MONTH_COMPARE: u64 = 0x5b;

    fn read_byte(rtc: &Dp8573a, address: u64) -> Result<u8, BusFault> {
        let mut value = [0xff];
        rtc.read(DeviceAddr::new(address), &mut value)?;
        Ok(value[0])
    }

    fn read_word(rtc: &Dp8573a, address: u64) -> Result<u32, BusFault> {
        let mut value = [0xff; 4];
        rtc.read(DeviceAddr::new(address), &mut value)?;
        Ok(u32::from_be_bytes(value))
    }

    #[test]
    fn compare_ram_stores_independent_values() {
        let mut rtc = Dp8573a::new();

        rtc.write(DeviceAddr::new(HOURS_COMPARE), &[0xa5]).unwrap();
        rtc.write(DeviceAddr::new(DAY_OF_MONTH_COMPARE), &[0x5a])
            .unwrap();

        assert_eq!(read_byte(&rtc, HOURS_COMPARE), Ok(0xa5));
        assert_eq!(read_byte(&rtc, DAY_OF_MONTH_COMPARE), Ok(0x5a));
    }

    #[test]
    fn aligned_words_use_the_low_byte_lane() {
        let mut rtc = Dp8573a::new();

        rtc.write(DeviceAddr::new(0x64), &0x1234_56a5_u32.to_be_bytes())
            .unwrap();

        assert_eq!(read_word(&rtc, 0x64), Ok(0xa5));
        assert_eq!(read_byte(&rtc, 0x67), Ok(0xa5));
    }

    #[test]
    fn main_status_selects_the_alternate_control_bank() {
        let mut rtc = Dp8573a::new();

        rtc.write(DeviceAddr::new(0x07), &[0x12]).unwrap();
        rtc.write(DeviceAddr::new(0x03), &[0x40]).unwrap();
        rtc.write(DeviceAddr::new(0x07), &[0x34]).unwrap();
        assert_eq!(read_byte(&rtc, 0x07), Ok(0x34));
        rtc.write(DeviceAddr::new(0x03), &[0]).unwrap();
        assert_eq!(read_byte(&rtc, 0x07), Ok(0x12));
    }

    #[test]
    fn rejects_invalid_lanes_widths_and_registers_atomically() {
        let mut rtc = Dp8573a::new();
        rtc.write(DeviceAddr::new(HOURS_COMPARE), &[0xa5]).unwrap();

        assert_eq!(
            rtc.write(DeviceAddr::new(0x54), &[0]),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(
            rtc.write(DeviceAddr::new(HOURS_COMPARE), &[1, 2]),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(
            rtc.write(DeviceAddr::new(0x80), &0_u32.to_be_bytes()),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(read_byte(&rtc, HOURS_COMPARE), Ok(0xa5));
    }
}
