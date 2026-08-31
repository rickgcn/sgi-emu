//! National Semiconductor DP8573A diagnostic RAM front end.

use se_core::bus::{BusFault, DeviceAddr};

const HOURS_COMPARE: u64 = 0x57;
const DAY_OF_MONTH_COMPARE: u64 = 0x5b;

/// The battery-backed compare bytes exercised by the IP12 diagnostics.
pub struct Dp8573a {
    hours_compare: u8,
    day_of_month_compare: u8,
}

impl Dp8573a {
    /// Creates an RTC front end with cleared compare RAM.
    #[must_use]
    #[allow(
        clippy::new_without_default,
        reason = "device construction is intentionally explicit"
    )]
    pub const fn new() -> Self {
        Self {
            hours_compare: 0,
            day_of_month_compare: 0,
        }
    }

    /// Reads one implemented compare byte.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault::UnsupportedAccess`] for unsupported registers or
    /// widths inside the decoded RTC aperture.
    pub fn read(&self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusFault> {
        let register = decode_register(address, data.len())?;
        data[0] = match register {
            Register::HoursCompare => self.hours_compare,
            Register::DayOfMonthCompare => self.day_of_month_compare,
        };
        Ok(())
    }

    /// Writes one implemented compare byte.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault::UnsupportedAccess`] for unsupported registers or
    /// widths inside the decoded RTC aperture.
    pub fn write(&mut self, address: DeviceAddr, data: &[u8]) -> Result<(), BusFault> {
        match decode_register(address, data.len())? {
            Register::HoursCompare => self.hours_compare = data[0],
            Register::DayOfMonthCompare => self.day_of_month_compare = data[0],
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Register {
    HoursCompare,
    DayOfMonthCompare,
}

fn decode_register(address: DeviceAddr, length: usize) -> Result<Register, BusFault> {
    if length != 1 {
        return Err(BusFault::UnsupportedAccess);
    }

    match address.get() {
        HOURS_COMPARE => Ok(Register::HoursCompare),
        DAY_OF_MONTH_COMPARE => Ok(Register::DayOfMonthCompare),
        _ => Err(BusFault::UnsupportedAccess),
    }
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusFault, DeviceAddr};

    use super::{DAY_OF_MONTH_COMPARE, Dp8573a, HOURS_COMPARE};

    fn read_byte(rtc: &Dp8573a, address: u64) -> Result<u8, BusFault> {
        let mut value = [0xff];
        rtc.read(DeviceAddr::new(address), &mut value)?;
        Ok(value[0])
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
    fn rejects_other_registers_and_widths_atomically() {
        let mut rtc = Dp8573a::new();
        rtc.write(DeviceAddr::new(HOURS_COMPARE), &[0xa5]).unwrap();

        assert_eq!(
            rtc.write(DeviceAddr::new(0), &[0]),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(
            rtc.write(DeviceAddr::new(HOURS_COMPARE), &[1, 2]),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(read_byte(&rtc, HOURS_COMPARE), Ok(0xa5));
    }
}
