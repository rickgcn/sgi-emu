//! SEEQ 8003 Ethernet controller register front end.

use se_core::bus::{BusError, DeviceAddr};
use serde::{Deserialize, Serialize};

const REGISTER_BYTES: u64 = 4;
const REGISTER_COUNT: u64 = 8;
const RECEIVE_COMMAND_SLOT: usize = 6;
const TRANSMIT_COMMAND_SLOT: usize = 7;
const BANK_SELECT_MASK: u8 = 0x60;
const OLD_DEVICE_STATUS: u8 = 0x80;

/// The software-visible SEEQ 8003 state used by the IP12 machine.
#[derive(Clone, Deserialize, Serialize)]
pub struct Seeq8003 {
    station_address: [u8; 6],
    multicast_low: [u8; 6],
    multicast_high: [u8; 2],
    inter_packet_gap: u8,
    control: u8,
    receive_command: u8,
    transmit_command: u8,
}

impl Seeq8003 {
    /// Creates a SEEQ 8003 in its reset state.
    #[must_use]
    #[allow(
        clippy::new_without_default,
        reason = "device construction is intentionally explicit"
    )]
    pub const fn new() -> Self {
        Self {
            station_address: [0; 6],
            multicast_low: [0; 6],
            multicast_high: [0; 2],
            inter_packet_gap: 0,
            control: 0,
            receive_command: 0,
            transmit_command: 0,
        }
    }

    /// Restores the mutable SEEQ 8003 reset state.
    pub fn reset(&mut self) {
        self.station_address = [0; 6];
        self.multicast_low = [0; 6];
        self.multicast_high = [0; 2];
        self.inter_packet_gap = 0;
        self.control = 0;
        self.receive_command = 0;
        self.transmit_command = 0;
    }

    /// Reads one fixed-width device-local transaction.
    ///
    /// # Errors
    ///
    /// Returns [`BusError`] when the complete transaction does not fit one
    /// external register or the width is unsupported.
    pub fn read(&self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusError> {
        let (slot, offset) = register_transaction(address, data.len())?;
        let value = match slot {
            0 => OLD_DEVICE_STATUS,
            1..=5 => 0,
            RECEIVE_COMMAND_SLOT => OLD_DEVICE_STATUS,
            TRANSMIT_COMMAND_SLOT => 0,
            _ => unreachable!("validated SEEQ register slot"),
        };
        let bytes = u32::from(value).to_be_bytes();
        data.copy_from_slice(&bytes[offset..offset + data.len()]);
        Ok(())
    }

    /// Writes one fixed-width device-local transaction.
    ///
    /// # Errors
    ///
    /// Returns [`BusError`] when the complete transaction does not fit one
    /// external register or the width is unsupported.
    pub fn write(&mut self, address: DeviceAddr, data: &[u8]) -> Result<(), BusError> {
        let (slot, offset) = register_transaction(address, data.len())?;
        let low_lane = 3;
        if offset > low_lane || offset + data.len() <= low_lane {
            return Ok(());
        }
        let value = data[low_lane - offset];

        match slot {
            0..=5 => self.write_banked_register(slot, value),
            RECEIVE_COMMAND_SLOT => self.receive_command = value,
            TRANSMIT_COMMAND_SLOT => self.transmit_command = value,
            _ => unreachable!("validated SEEQ register slot"),
        }
        Ok(())
    }

    fn write_banked_register(&mut self, slot: usize, value: u8) {
        match self.transmit_command & BANK_SELECT_MASK {
            0x00 => self.station_address[slot] = value,
            0x20 => self.multicast_low[slot] = value,
            0x40 => match slot {
                0..=1 => self.multicast_high[slot] = value,
                2 => self.inter_packet_gap = value,
                3 => self.control = value,
                4..=5 => {}
                _ => unreachable!("validated banked SEEQ register slot"),
            },
            _ => {}
        }
    }
}

fn register_transaction(address: DeviceAddr, length: usize) -> Result<(usize, usize), BusError> {
    if !(1..=4).contains(&length) {
        return Err(BusError::InvalidTransaction);
    }

    let start = address.get();
    let length = u64::try_from(length).map_err(|_| BusError::InvalidTransaction)?;
    let end = start
        .checked_add(length)
        .ok_or(BusError::InvalidTransaction)?;
    let register_end = REGISTER_COUNT * REGISTER_BYTES;
    if start >= register_end || end > register_end {
        return Err(BusError::HardwareFault);
    }
    if start / REGISTER_BYTES != (end - 1) / REGISTER_BYTES {
        return Err(BusError::UnimplementedAccess);
    }

    let slot =
        usize::try_from(start / REGISTER_BYTES).map_err(|_| BusError::UnimplementedAccess)?;
    let offset =
        usize::try_from(start % REGISTER_BYTES).map_err(|_| BusError::UnimplementedAccess)?;
    Ok((slot, offset))
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusError, DeviceAddr};

    use super::{OLD_DEVICE_STATUS, Seeq8003};

    fn write_word(seeq: &mut Seeq8003, slot: u64, value: u8) {
        seeq.write(DeviceAddr::new(slot * 4), &u32::from(value).to_be_bytes())
            .unwrap();
    }

    fn read_word(seeq: &Seeq8003, slot: u64) -> Result<u32, BusError> {
        let mut data = [0; 4];
        seeq.read(DeviceAddr::new(slot * 4), &mut data)?;
        Ok(u32::from_be_bytes(data))
    }

    #[test]
    fn reset_selects_station_address_bank_and_clears_writable_state() {
        let mut seeq = Seeq8003::new();
        for (slot, value) in [0x08, 0x00, 0x69, 0x12, 0x34, 0x56].into_iter().enumerate() {
            write_word(&mut seeq, slot as u64, value);
        }
        write_word(&mut seeq, 6, 0x9f);
        write_word(&mut seeq, 7, 0x4f);

        seeq.reset();

        assert_eq!(seeq.station_address, [0; 6]);
        assert_eq!(seeq.multicast_low, [0; 6]);
        assert_eq!(seeq.multicast_high, [0; 2]);
        assert_eq!(seeq.inter_packet_gap, 0);
        assert_eq!(seeq.control, 0);
        assert_eq!(seeq.receive_command, 0);
        assert_eq!(seeq.transmit_command, 0);
    }

    #[test]
    fn transmit_command_selects_three_independent_write_banks() {
        let mut seeq = Seeq8003::new();

        for slot in 0..6 {
            write_word(&mut seeq, slot, 0x10 + slot as u8);
        }
        write_word(&mut seeq, 7, 0x20);
        for slot in 0..6 {
            write_word(&mut seeq, slot, 0x20 + slot as u8);
        }
        write_word(&mut seeq, 7, 0x40);
        for (slot, value) in [0x31, 0x32, 0x33, 0x34, 0x35, 0x36].into_iter().enumerate() {
            write_word(&mut seeq, slot as u64, value);
        }

        assert_eq!(seeq.station_address, [0x10, 0x11, 0x12, 0x13, 0x14, 0x15]);
        assert_eq!(seeq.multicast_low, [0x20, 0x21, 0x22, 0x23, 0x24, 0x25]);
        assert_eq!(seeq.multicast_high, [0x31, 0x32]);
        assert_eq!(seeq.inter_packet_gap, 0x33);
        assert_eq!(seeq.control, 0x34);
    }

    #[test]
    fn command_aliases_share_the_external_low_byte_lanes() {
        let mut seeq = Seeq8003::new();

        seeq.write(DeviceAddr::new(0x1b), &[0xa5]).unwrap();
        seeq.write(DeviceAddr::new(0x1f), &[0x24]).unwrap();

        assert_eq!(seeq.receive_command, 0xa5);
        assert_eq!(seeq.transmit_command, 0x24);
    }

    #[test]
    fn old_seeq_probe_and_status_use_the_low_byte_lane() {
        let seeq = Seeq8003::new();

        assert_eq!(read_word(&seeq, 0), Ok(u32::from(OLD_DEVICE_STATUS)));
        assert_eq!(read_word(&seeq, 6), Ok(u32::from(OLD_DEVICE_STATUS)));
        let mut high_lane = [0xff];
        seeq.read(DeviceAddr::new(0), &mut high_lane).unwrap();
        assert_eq!(high_lane, [0]);
        let mut low_lane = [0];
        seeq.read(DeviceAddr::new(3), &mut low_lane).unwrap();
        assert_eq!(low_lane, [OLD_DEVICE_STATUS]);
    }

    #[test]
    fn rejects_crossing_unmapped_and_unsupported_transactions() {
        let mut seeq = Seeq8003::new();

        assert_eq!(
            seeq.read(DeviceAddr::new(3), &mut [0; 2]),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(
            seeq.read(DeviceAddr::new(0x20), &mut [0]),
            Err(BusError::HardwareFault)
        );
        assert_eq!(
            seeq.write(DeviceAddr::new(0), &[]),
            Err(BusError::InvalidTransaction)
        );
        assert_eq!(
            seeq.write(DeviceAddr::new(0x20), &[0]),
            Err(BusError::HardwareFault)
        );
    }
}
