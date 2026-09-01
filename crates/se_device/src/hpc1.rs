//! Silicon Graphics HPC1.5 register front end.

use se_core::bus::{BusFault, DeviceAddr};

const ETHERNET_TRANSMIT_STATUS: u64 = 0x0034;
const ETHERNET_RECEIVE_STATUS: u64 = 0x0038;
const ETHERNET_RESET: u64 = 0x003c;
const ETHERNET_RECEIVE_POINTER: u64 = 0x0058;
const ETHERNET_RECEIVE_FIFO: u64 = 0x005c;
const SCSI_CONTROL: u64 = 0x0094;
const ENDIAN_CONTROL: u64 = 0x00c0;
const MISCELLANEOUS_CONTROL: u64 = 0x01b0;
const REGISTER_BYTES: u64 = 4;

const REVISION: u8 = 0x40;
const WRITABLE_ENDIAN_BITS: u8 = 0x1f;
const WRITABLE_SCSI_BITS: u8 = 0x93;
const SCSI_RESET: u8 = 0x01;
const ETHERNET_RESET_CHANNEL: u32 = 0x01;
const ETHERNET_TRANSMIT_STATUS_BITS: u32 = 0x00ff_0000;
const ETHERNET_RECEIVE_STATUS_BITS: u32 = 0x0000_ff00;
const ETHERNET_CONTROL_BITS: u32 = 0x0f;

/// The software-visible HPC1.5 state used by the IP12 machine.
pub struct Hpc1 {
    ethernet_transmit_status: u32,
    ethernet_receive_status: u32,
    ethernet_control: u32,
    ethernet_receive_pointer: u8,
    scsi_control: u8,
    endian_control: u8,
    miscellaneous_control: u32,
    scsi_reset_requested: bool,
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
            ethernet_transmit_status: 0,
            ethernet_receive_status: 0,
            ethernet_control: 0,
            ethernet_receive_pointer: 0,
            scsi_control: 0,
            endian_control: REVISION,
            miscellaneous_control: 0,
            scsi_reset_requested: false,
        }
    }

    /// Restores the mutable HPC1.5 reset state.
    pub fn reset(&mut self) {
        self.ethernet_transmit_status = 0;
        self.ethernet_receive_status = 0;
        self.ethernet_control = 0;
        self.ethernet_receive_pointer = 0;
        self.scsi_control = 0;
        self.endian_control = REVISION;
        self.miscellaneous_control = 0;
        self.scsi_reset_requested = false;
    }

    /// Reads one fixed-width device-local transaction.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault`] when the complete transaction is not mapped or the
    /// requested width or direction is unsupported.
    pub fn read(&self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusFault> {
        let (start, end) = transaction_bounds(address, data.len())?;

        if let Some(offset) = register_offset(start, end, ETHERNET_TRANSMIT_STATUS) {
            read_register(self.ethernet_transmit_status, offset, data);
        } else if let Some(offset) = register_offset(start, end, ETHERNET_RECEIVE_STATUS) {
            read_register(self.ethernet_receive_status, offset, data);
        } else if let Some(offset) = register_offset(start, end, ETHERNET_RESET) {
            read_register(self.ethernet_control, offset, data);
        } else if start == ETHERNET_RECEIVE_POINTER + 3 && end == start + 1 {
            data[0] = self.ethernet_receive_pointer;
        } else if start == ETHERNET_RECEIVE_FIFO + 3 && end == start + 1 {
            data[0] = 0;
        } else if let Some(offset) = register_offset(start, end, SCSI_CONTROL) {
            read_register(u32::from(self.scsi_control), offset, data);
        } else if let Some(offset) = register_offset(start, end, ENDIAN_CONTROL) {
            read_register(u32::from(self.endian_control), offset, data);
        } else if start == MISCELLANEOUS_CONTROL && end == start + REGISTER_BYTES {
            data.copy_from_slice(&self.miscellaneous_control.to_be_bytes());
        } else if contains_register(start, end, ETHERNET_RECEIVE_POINTER)
            || contains_register(start, end, ETHERNET_RECEIVE_FIFO)
            || contains_register(start, end, MISCELLANEOUS_CONTROL)
        {
            return Err(BusFault::UnsupportedAccess);
        } else {
            return Err(BusFault::Unmapped);
        }

        Ok(())
    }

    /// Writes one fixed-width device-local transaction.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault`] when the complete transaction is not mapped or the
    /// requested width or direction is unsupported.
    pub fn write(&mut self, address: DeviceAddr, data: &[u8]) -> Result<(), BusFault> {
        let (start, end) = transaction_bounds(address, data.len())?;

        if let Some(offset) = register_offset(start, end, ETHERNET_TRANSMIT_STATUS) {
            let value = write_register(self.ethernet_transmit_status, offset, data);
            self.ethernet_transmit_status = value & ETHERNET_TRANSMIT_STATUS_BITS;
        } else if let Some(offset) = register_offset(start, end, ETHERNET_RECEIVE_STATUS) {
            let value = write_register(self.ethernet_receive_status, offset, data);
            self.ethernet_receive_status = value & ETHERNET_RECEIVE_STATUS_BITS;
        } else if let Some(offset) = register_offset(start, end, ETHERNET_RESET) {
            let value = write_register(self.ethernet_control, offset, data);
            self.ethernet_control = value & ETHERNET_CONTROL_BITS;
            if self.ethernet_control & ETHERNET_RESET_CHANNEL != 0 {
                self.ethernet_transmit_status = 0;
                self.ethernet_receive_status = 0;
            }
        } else if start == ETHERNET_RECEIVE_POINTER + 3 && end == start + 1 {
            self.ethernet_receive_pointer = data[0];
        } else if contains_register(start, end, ETHERNET_RECEIVE_FIFO) {
            return Err(BusFault::UnsupportedAccess);
        } else if let Some(offset) = register_offset(start, end, SCSI_CONTROL) {
            let old_value = self.scsi_control;
            let value = write_register(u32::from(old_value), offset, data) as u8;
            self.scsi_control = value & WRITABLE_SCSI_BITS;
            self.scsi_reset_requested |=
                old_value & SCSI_RESET == 0 && self.scsi_control & SCSI_RESET != 0;
        } else if let Some(offset) = register_offset(start, end, ENDIAN_CONTROL) {
            let value = write_register(u32::from(self.endian_control), offset, data) as u8;
            self.endian_control = REVISION | (value & WRITABLE_ENDIAN_BITS);
        } else if start == MISCELLANEOUS_CONTROL && end == start + REGISTER_BYTES {
            self.miscellaneous_control =
                u32::from_be_bytes(data.try_into().map_err(|_| BusFault::UnsupportedAccess)?);
        } else if contains_register(start, end, ETHERNET_RECEIVE_POINTER)
            || contains_register(start, end, MISCELLANEOUS_CONTROL)
        {
            return Err(BusFault::UnsupportedAccess);
        } else {
            return Err(BusFault::Unmapped);
        }

        Ok(())
    }

    /// Returns and clears a pending reset request for the attached SCSI controller.
    pub fn take_scsi_reset_request(&mut self) -> bool {
        let requested = self.scsi_reset_requested;
        self.scsi_reset_requested = false;
        requested
    }
}

fn transaction_bounds(address: DeviceAddr, length: usize) -> Result<(u64, u64), BusFault> {
    if !(1..=4).contains(&length) {
        return Err(BusFault::UnsupportedAccess);
    }

    let start = address.get();
    let length = u64::try_from(length).map_err(|_| BusFault::UnsupportedAccess)?;
    let end = start.checked_add(length).ok_or(BusFault::Unmapped)?;
    Ok((start, end))
}

fn register_offset(start: u64, end: u64, register: u64) -> Option<usize> {
    if !contains_register(start, end, register) {
        return None;
    }

    usize::try_from(start - register).ok()
}

const fn contains_register(start: u64, end: u64, register: u64) -> bool {
    start >= register && end <= register + REGISTER_BYTES
}

fn read_register(value: u32, offset: usize, data: &mut [u8]) {
    data.copy_from_slice(&value.to_be_bytes()[offset..offset + data.len()]);
}

fn write_register(value: u32, offset: usize, data: &[u8]) -> u32 {
    let mut bytes = value.to_be_bytes();
    bytes[offset..offset + data.len()].copy_from_slice(data);
    u32::from_be_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusFault, DeviceAddr};

    use super::{
        ENDIAN_CONTROL, ETHERNET_RECEIVE_FIFO, ETHERNET_RECEIVE_POINTER, ETHERNET_RECEIVE_STATUS,
        ETHERNET_RESET, ETHERNET_TRANSMIT_STATUS, Hpc1, MISCELLANEOUS_CONTROL, SCSI_CONTROL,
    };

    fn read_word(hpc1: &Hpc1, address: u64) -> Result<u32, BusFault> {
        let mut bytes = [0; 4];
        hpc1.read(DeviceAddr::new(address), &mut bytes)?;
        Ok(u32::from_be_bytes(bytes))
    }

    #[test]
    fn reset_values_match_the_ip12_front_end() {
        let hpc1 = Hpc1::new();

        assert_eq!(read_word(&hpc1, SCSI_CONTROL), Ok(0));
        assert_eq!(read_word(&hpc1, ENDIAN_CONTROL), Ok(0x40));
        assert_eq!(read_word(&hpc1, MISCELLANEOUS_CONTROL), Ok(0));
        let mut byte = [0xff];
        assert_eq!(
            hpc1.read(DeviceAddr::new(ETHERNET_RECEIVE_POINTER + 3), &mut byte),
            Ok(())
        );
        assert_eq!(byte, [0]);
    }

    #[test]
    fn endian_and_scsi_controls_use_big_endian_lanes_and_masks() {
        let mut hpc1 = Hpc1::new();

        hpc1.write(DeviceAddr::new(ENDIAN_CONTROL), &[0xff; 4])
            .unwrap();
        hpc1.write(DeviceAddr::new(SCSI_CONTROL), &[0xff; 4])
            .unwrap();

        assert_eq!(read_word(&hpc1, ENDIAN_CONTROL), Ok(0x5f));
        assert_eq!(read_word(&hpc1, SCSI_CONTROL), Ok(0x93));
    }

    #[test]
    fn scsi_reset_requests_are_edge_triggered() {
        let mut hpc1 = Hpc1::new();

        hpc1.write(DeviceAddr::new(SCSI_CONTROL + 3), &[1]).unwrap();
        assert!(hpc1.take_scsi_reset_request());
        assert!(!hpc1.take_scsi_reset_request());
        hpc1.write(DeviceAddr::new(SCSI_CONTROL + 3), &[1]).unwrap();
        assert!(!hpc1.take_scsi_reset_request());
        hpc1.write(DeviceAddr::new(SCSI_CONTROL + 3), &[0]).unwrap();
        hpc1.write(DeviceAddr::new(SCSI_CONTROL + 3), &[1]).unwrap();
        assert!(hpc1.take_scsi_reset_request());
    }

    #[test]
    fn ethernet_diagnostic_state_uses_only_the_low_byte_lane() {
        let mut hpc1 = Hpc1::new();
        let mut byte = [0xff];

        hpc1.write(DeviceAddr::new(ETHERNET_RECEIVE_POINTER + 3), &[0x5a])
            .unwrap();
        hpc1.read(DeviceAddr::new(ETHERNET_RECEIVE_POINTER + 3), &mut byte)
            .unwrap();
        assert_eq!(byte, [0x5a]);

        hpc1.read(DeviceAddr::new(ETHERNET_RECEIVE_FIFO + 3), &mut byte)
            .unwrap();
        assert_eq!(byte, [0]);
        assert_eq!(
            hpc1.write(DeviceAddr::new(ETHERNET_RECEIVE_FIFO + 3), &[1]),
            Err(BusFault::UnsupportedAccess)
        );
    }

    #[test]
    fn ethernet_status_and_reset_use_the_documented_word_lanes() {
        let mut hpc1 = Hpc1::new();

        hpc1.write(
            DeviceAddr::new(ETHERNET_TRANSMIT_STATUS),
            &0xffff_ffff_u32.to_be_bytes(),
        )
        .unwrap();
        hpc1.write(
            DeviceAddr::new(ETHERNET_RECEIVE_STATUS),
            &0xffff_ffff_u32.to_be_bytes(),
        )
        .unwrap();
        assert_eq!(read_word(&hpc1, ETHERNET_TRANSMIT_STATUS), Ok(0x00ff_0000));
        assert_eq!(read_word(&hpc1, ETHERNET_RECEIVE_STATUS), Ok(0x0000_ff00));

        hpc1.write(DeviceAddr::new(ETHERNET_RESET), &5_u32.to_be_bytes())
            .unwrap();
        assert_eq!(read_word(&hpc1, ETHERNET_TRANSMIT_STATUS), Ok(0));
        assert_eq!(read_word(&hpc1, ETHERNET_RECEIVE_STATUS), Ok(0));
        assert_eq!(read_word(&hpc1, ETHERNET_RESET), Ok(5));

        hpc1.write(DeviceAddr::new(ETHERNET_RESET), &4_u32.to_be_bytes())
            .unwrap();
        assert_eq!(read_word(&hpc1, ETHERNET_RESET), Ok(4));
    }

    #[test]
    fn miscellaneous_control_requires_a_complete_word() {
        let mut hpc1 = Hpc1::new();

        hpc1.write(DeviceAddr::new(MISCELLANEOUS_CONTROL), &9_u32.to_be_bytes())
            .unwrap();
        assert_eq!(read_word(&hpc1, MISCELLANEOUS_CONTROL), Ok(9));
        assert_eq!(
            hpc1.write(DeviceAddr::new(MISCELLANEOUS_CONTROL + 3), &[0]),
            Err(BusFault::UnsupportedAccess)
        );
    }

    #[test]
    fn reset_clears_mutable_state() {
        let mut hpc1 = Hpc1::new();
        hpc1.write(DeviceAddr::new(ETHERNET_RECEIVE_POINTER + 3), &[0x5a])
            .unwrap();
        hpc1.write(DeviceAddr::new(SCSI_CONTROL + 3), &[0x93])
            .unwrap();
        hpc1.write(DeviceAddr::new(ENDIAN_CONTROL + 3), &[0x1f])
            .unwrap();
        hpc1.write(DeviceAddr::new(MISCELLANEOUS_CONTROL), &9_u32.to_be_bytes())
            .unwrap();

        hpc1.reset();

        assert_eq!(read_word(&hpc1, SCSI_CONTROL), Ok(0));
        assert_eq!(read_word(&hpc1, ENDIAN_CONTROL), Ok(0x40));
        assert_eq!(read_word(&hpc1, MISCELLANEOUS_CONTROL), Ok(0));
        assert!(!hpc1.take_scsi_reset_request());
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
        assert_eq!(read_word(&hpc1, ENDIAN_CONTROL), Ok(0x52));
    }
}
