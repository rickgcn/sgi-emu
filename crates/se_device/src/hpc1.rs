//! Silicon Graphics HPC1.5 register front end.

use se_core::bus::{BusFault, DeviceAddr};
use se_core::time::{ATTOSECONDS_PER_SECOND, VirtualDuration};

const ETHERNET_TRANSMIT_STATUS: u64 = 0x0034;
const ETHERNET_RECEIVE_STATUS: u64 = 0x0038;
const ETHERNET_RESET: u64 = 0x003c;
const ETHERNET_RECEIVE_POINTER: u64 = 0x0058;
const ETHERNET_RECEIVE_FIFO: u64 = 0x005c;
const SCSI_BYTE_COUNT: u64 = 0x0088;
const SCSI_CURRENT_BUFFER_POINTER: u64 = 0x008c;
const SCSI_NEXT_DESCRIPTOR_POINTER: u64 = 0x0090;
const SCSI_CONTROL: u64 = 0x0094;
const SCSI_FIFO_POINTER: u64 = 0x0098;
const ENDIAN_CONTROL: u64 = 0x00c0;
const FREE_RUNNING_COUNTER: u64 = 0x0194;
const MISCELLANEOUS_CONTROL: u64 = 0x01b0;
const REGISTER_BYTES: u64 = 4;

const REVISION: u8 = 0x40;
const WRITABLE_ENDIAN_BITS: u8 = 0x1f;
const WRITABLE_SCSI_BITS: u8 = 0x93;
const SCSI_RESET: u8 = 0x01;
const SCSI_TO_MEMORY: u8 = 0x10;
const SCSI_START_DMA: u8 = 0x80;
const SCSI_BYTE_COUNT_MASK: u16 = 0x1fff;
const SCSI_ADDRESS_MASK: u32 = 0x0fff_ffff;
const SCSI_DESCRIPTOR_END: u32 = 1 << 31;
const ETHERNET_RESET_CHANNEL: u32 = 0x01;
const ETHERNET_TRANSMIT_STATUS_BITS: u32 = 0x00ff_0000;
const ETHERNET_RECEIVE_STATUS_BITS: u32 = 0x0000_ff00;
const ETHERNET_CONTROL_BITS: u32 = 0x0f;
const FREE_RUNNING_COUNTER_FREQUENCY: u128 = 33_000_000;
const FREE_RUNNING_COUNTER_MODULUS: u128 = 1 << 24;

/// One currently available HPC1 SCSI DMA window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScsiDmaWindow {
    buffer_address: u32,
    byte_count: u16,
    to_memory: bool,
}

impl ScsiDmaWindow {
    /// Returns the physical buffer address.
    #[must_use]
    pub const fn buffer_address(&self) -> u32 {
        self.buffer_address
    }

    /// Returns the number of bytes available in this descriptor.
    #[must_use]
    pub const fn byte_count(&self) -> u16 {
        self.byte_count
    }

    /// Reports whether bytes move from the SCSI target into memory.
    #[must_use]
    pub const fn to_memory(&self) -> bool {
        self.to_memory
    }
}

/// The software-visible HPC1.5 state used by the IP12 machine.
pub struct Hpc1 {
    ethernet_transmit_status: u32,
    ethernet_receive_status: u32,
    ethernet_control: u32,
    ethernet_receive_pointer: u8,
    scsi_byte_count: u16,
    scsi_current_buffer_pointer: u32,
    scsi_next_descriptor_pointer: u32,
    scsi_descriptor_end: bool,
    scsi_descriptor_loaded: bool,
    scsi_descriptor_fetch_pending: bool,
    scsi_control: u8,
    endian_control: u8,
    free_running_counter: u32,
    free_running_counter_phase: u128,
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
            scsi_byte_count: 0,
            scsi_current_buffer_pointer: 0,
            scsi_next_descriptor_pointer: 0,
            scsi_descriptor_end: false,
            scsi_descriptor_loaded: false,
            scsi_descriptor_fetch_pending: false,
            scsi_control: 0,
            endian_control: REVISION,
            free_running_counter: 0,
            free_running_counter_phase: 0,
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
        self.scsi_byte_count = 0;
        self.scsi_current_buffer_pointer = 0;
        self.scsi_next_descriptor_pointer = 0;
        self.scsi_descriptor_end = false;
        self.scsi_descriptor_loaded = false;
        self.scsi_descriptor_fetch_pending = false;
        self.scsi_control = 0;
        self.endian_control = REVISION;
        self.free_running_counter = 0;
        self.free_running_counter_phase = 0;
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
        } else if let Some(offset) = register_offset(start, end, SCSI_BYTE_COUNT) {
            read_register(u32::from(self.scsi_byte_count), offset, data);
        } else if let Some(offset) = register_offset(start, end, SCSI_CURRENT_BUFFER_POINTER) {
            read_register(self.scsi_current_buffer_pointer, offset, data);
        } else if let Some(offset) = register_offset(start, end, SCSI_NEXT_DESCRIPTOR_POINTER) {
            read_register(self.scsi_next_descriptor_pointer, offset, data);
        } else if let Some(offset) = register_offset(start, end, SCSI_CONTROL) {
            read_register(u32::from(self.scsi_control), offset, data);
        } else if let Some(offset) = register_offset(start, end, SCSI_FIFO_POINTER) {
            read_register(0, offset, data);
        } else if let Some(offset) = register_offset(start, end, ENDIAN_CONTROL) {
            read_register(u32::from(self.endian_control), offset, data);
        } else if start == FREE_RUNNING_COUNTER && end == start + REGISTER_BYTES {
            data.copy_from_slice(&self.free_running_counter.to_be_bytes());
        } else if start == MISCELLANEOUS_CONTROL && end == start + REGISTER_BYTES {
            data.copy_from_slice(&self.miscellaneous_control.to_be_bytes());
        } else if contains_register(start, end, ETHERNET_RECEIVE_POINTER)
            || contains_register(start, end, ETHERNET_RECEIVE_FIFO)
            || contains_register(start, end, FREE_RUNNING_COUNTER)
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
        } else if let Some(offset) = register_offset(start, end, SCSI_BYTE_COUNT) {
            let value = write_register(u32::from(self.scsi_byte_count), offset, data);
            self.scsi_byte_count = value as u16 & SCSI_BYTE_COUNT_MASK;
        } else if let Some(offset) = register_offset(start, end, SCSI_CURRENT_BUFFER_POINTER) {
            self.scsi_current_buffer_pointer =
                write_register(self.scsi_current_buffer_pointer, offset, data) & SCSI_ADDRESS_MASK;
        } else if let Some(offset) = register_offset(start, end, SCSI_NEXT_DESCRIPTOR_POINTER) {
            self.scsi_next_descriptor_pointer =
                write_register(self.scsi_next_descriptor_pointer, offset, data) & SCSI_ADDRESS_MASK;
            self.scsi_descriptor_loaded = false;
            self.scsi_descriptor_fetch_pending = true;
        } else if let Some(offset) = register_offset(start, end, SCSI_CONTROL) {
            let old_value = self.scsi_control;
            let value = write_register(u32::from(old_value), offset, data) as u8;
            self.scsi_control = value & WRITABLE_SCSI_BITS;
            self.scsi_reset_requested |=
                old_value & SCSI_RESET == 0 && self.scsi_control & SCSI_RESET != 0;
            if old_value & SCSI_START_DMA == 0
                && self.scsi_control & SCSI_START_DMA != 0
                && !self.scsi_descriptor_loaded
            {
                self.scsi_descriptor_fetch_pending = true;
            }
        } else if contains_register(start, end, SCSI_FIFO_POINTER) {
            return Err(BusFault::UnsupportedAccess);
        } else if let Some(offset) = register_offset(start, end, ENDIAN_CONTROL) {
            let value = write_register(u32::from(self.endian_control), offset, data) as u8;
            self.endian_control = REVISION | (value & WRITABLE_ENDIAN_BITS);
        } else if contains_register(start, end, FREE_RUNNING_COUNTER) {
            return Err(BusFault::UnsupportedAccess);
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

    /// Advances the 33 MHz free-running counter by guest virtual time.
    pub fn advance_time(&mut self, elapsed: VirtualDuration) {
        let attoseconds = elapsed.as_attoseconds();
        let whole_seconds = attoseconds / ATTOSECONDS_PER_SECOND;
        let partial_attoseconds = attoseconds % ATTOSECONDS_PER_SECOND;
        let scaled_partial =
            partial_attoseconds * FREE_RUNNING_COUNTER_FREQUENCY + self.free_running_counter_phase;
        let partial_ticks = scaled_partial / ATTOSECONDS_PER_SECOND;
        self.free_running_counter_phase = scaled_partial % ATTOSECONDS_PER_SECOND;

        let whole_ticks = (whole_seconds % FREE_RUNNING_COUNTER_MODULUS)
            * (FREE_RUNNING_COUNTER_FREQUENCY % FREE_RUNNING_COUNTER_MODULUS);
        let elapsed_ticks = (whole_ticks + partial_ticks % FREE_RUNNING_COUNTER_MODULUS)
            % FREE_RUNNING_COUNTER_MODULUS;
        self.free_running_counter = ((u128::from(self.free_running_counter) + elapsed_ticks)
            % FREE_RUNNING_COUNTER_MODULUS) as u32;
    }

    /// Returns and clears a pending reset request for the attached SCSI controller.
    pub fn take_scsi_reset_request(&mut self) -> bool {
        let requested = self.scsi_reset_requested;
        self.scsi_reset_requested = false;
        requested
    }

    /// Returns and clears a pending SCSI descriptor fetch request.
    pub fn take_scsi_descriptor_fetch(&mut self) -> Option<u32> {
        if !self.scsi_descriptor_fetch_pending {
            return None;
        }
        self.scsi_descriptor_fetch_pending = false;
        Some(self.scsi_next_descriptor_pointer)
    }

    /// Loads one big-endian 12-byte SCSI DMA descriptor.
    pub fn load_scsi_descriptor(&mut self, descriptor: [u8; 12]) {
        let count = u32::from_be_bytes(descriptor[0..4].try_into().expect("fixed descriptor word"));
        let buffer =
            u32::from_be_bytes(descriptor[4..8].try_into().expect("fixed descriptor word"));
        let next = u32::from_be_bytes(descriptor[8..12].try_into().expect("fixed descriptor word"));
        self.scsi_byte_count = count as u16 & SCSI_BYTE_COUNT_MASK;
        self.scsi_current_buffer_pointer = buffer & SCSI_ADDRESS_MASK;
        self.scsi_next_descriptor_pointer = next & SCSI_ADDRESS_MASK;
        self.scsi_descriptor_end = buffer & SCSI_DESCRIPTOR_END != 0;
        self.scsi_descriptor_loaded = true;
        self.scsi_descriptor_fetch_pending = false;
    }

    /// Returns the active SCSI DMA buffer window.
    #[must_use]
    pub const fn scsi_dma_window(&self) -> Option<ScsiDmaWindow> {
        if self.scsi_control & SCSI_START_DMA == 0
            || !self.scsi_descriptor_loaded
            || self.scsi_byte_count == 0
        {
            return None;
        }
        Some(ScsiDmaWindow {
            buffer_address: self.scsi_current_buffer_pointer,
            byte_count: self.scsi_byte_count,
            to_memory: self.scsi_control & SCSI_TO_MEMORY != 0,
        })
    }

    /// Advances the active DMA cursor after one bulk transfer.
    ///
    /// Returns `false` without modifying state when `byte_count` exceeds the
    /// current descriptor.
    pub fn consume_scsi_dma_bytes(&mut self, byte_count: u16) -> bool {
        if byte_count > self.scsi_byte_count {
            return false;
        }
        self.scsi_byte_count -= byte_count;
        self.scsi_current_buffer_pointer = self
            .scsi_current_buffer_pointer
            .wrapping_add(u32::from(byte_count))
            & SCSI_ADDRESS_MASK;
        if self.scsi_byte_count == 0 {
            self.scsi_descriptor_loaded = false;
            if self.scsi_descriptor_end {
                self.scsi_control &= !SCSI_START_DMA;
            } else {
                self.scsi_descriptor_fetch_pending = true;
            }
        }
        true
    }

    /// Finishes a short target transfer while preserving descriptor residuals.
    pub fn finish_scsi_dma(&mut self) {
        self.scsi_control &= !SCSI_START_DMA;
        self.scsi_descriptor_fetch_pending = false;
    }

    /// Stops SCSI DMA after a descriptor, address, or protocol failure.
    pub fn stop_scsi_dma(&mut self) {
        self.scsi_control &= !SCSI_START_DMA;
        self.scsi_descriptor_fetch_pending = false;
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
    use se_core::time::{ATTOSECONDS_PER_SECOND, VirtualDuration};

    use super::{
        ENDIAN_CONTROL, ETHERNET_RECEIVE_FIFO, ETHERNET_RECEIVE_POINTER, ETHERNET_RECEIVE_STATUS,
        ETHERNET_RESET, ETHERNET_TRANSMIT_STATUS, FREE_RUNNING_COUNTER,
        FREE_RUNNING_COUNTER_FREQUENCY, FREE_RUNNING_COUNTER_MODULUS, Hpc1, MISCELLANEOUS_CONTROL,
        SCSI_BYTE_COUNT, SCSI_CONTROL, SCSI_CURRENT_BUFFER_POINTER, SCSI_FIFO_POINTER,
        SCSI_NEXT_DESCRIPTOR_POINTER,
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
        assert_eq!(read_word(&hpc1, FREE_RUNNING_COUNTER), Ok(0));
        assert_eq!(read_word(&hpc1, MISCELLANEOUS_CONTROL), Ok(0));
        let mut byte = [0xff];
        assert_eq!(
            hpc1.read(DeviceAddr::new(ETHERNET_RECEIVE_POINTER + 3), &mut byte),
            Ok(())
        );
        assert_eq!(byte, [0]);
    }

    #[test]
    fn free_running_counter_accumulates_exact_fractional_ticks_and_wraps() {
        let mut hpc1 = Hpc1::new();
        let almost_one_tick = ATTOSECONDS_PER_SECOND / FREE_RUNNING_COUNTER_FREQUENCY;

        hpc1.advance_time(VirtualDuration::from_attoseconds(almost_one_tick));
        assert_eq!(read_word(&hpc1, FREE_RUNNING_COUNTER), Ok(0));
        hpc1.advance_time(VirtualDuration::from_attoseconds(1));
        assert_eq!(read_word(&hpc1, FREE_RUNNING_COUNTER), Ok(1));

        hpc1.reset();
        hpc1.advance_time(VirtualDuration::from_attoseconds(
            2 * ATTOSECONDS_PER_SECOND,
        ));
        assert_eq!(
            read_word(&hpc1, FREE_RUNNING_COUNTER),
            Ok((2 * FREE_RUNNING_COUNTER_FREQUENCY % FREE_RUNNING_COUNTER_MODULUS) as u32)
        );
    }

    #[test]
    fn free_running_counter_is_a_read_only_word() {
        let mut hpc1 = Hpc1::new();

        assert_eq!(
            hpc1.read(DeviceAddr::new(FREE_RUNNING_COUNTER + 3), &mut [0]),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(
            hpc1.write(DeviceAddr::new(FREE_RUNNING_COUNTER), &0_u32.to_be_bytes()),
            Err(BusFault::UnsupportedAccess)
        );
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
    fn functional_scsi_dma_exposes_an_empty_fifo() {
        let mut hpc1 = Hpc1::new();

        assert_eq!(read_word(&hpc1, SCSI_FIFO_POINTER), Ok(0));
        assert_eq!(
            hpc1.write(DeviceAddr::new(SCSI_FIFO_POINTER), &0_u32.to_be_bytes()),
            Err(BusFault::UnsupportedAccess)
        );
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
    fn scsi_descriptor_walk_updates_the_bulk_dma_cursor() {
        let mut hpc1 = Hpc1::new();
        hpc1.write(
            DeviceAddr::new(SCSI_NEXT_DESCRIPTOR_POINTER),
            &0x0012_3400_u32.to_be_bytes(),
        )
        .unwrap();
        assert_eq!(hpc1.take_scsi_descriptor_fetch(), Some(0x0012_3400));
        assert_eq!(hpc1.take_scsi_descriptor_fetch(), None);

        let mut descriptor = [0; 12];
        descriptor[0..4].copy_from_slice(&0x0000_0200_u32.to_be_bytes());
        descriptor[4..8].copy_from_slice(&0x0010_0000_u32.to_be_bytes());
        descriptor[8..12].copy_from_slice(&0x0012_3500_u32.to_be_bytes());
        hpc1.load_scsi_descriptor(descriptor);
        hpc1.write(DeviceAddr::new(SCSI_CONTROL + 3), &[0x90])
            .unwrap();

        let window = hpc1.scsi_dma_window().unwrap();
        assert_eq!(window.buffer_address(), 0x0010_0000);
        assert_eq!(window.byte_count(), 512);
        assert!(window.to_memory());
        assert!(hpc1.consume_scsi_dma_bytes(512));
        assert_eq!(hpc1.take_scsi_descriptor_fetch(), Some(0x0012_3500));
    }

    #[test]
    fn end_of_chain_and_short_completion_clear_start_without_hiding_residuals() {
        let mut hpc1 = Hpc1::new();
        let mut descriptor = [0; 12];
        descriptor[0..4].copy_from_slice(&32_u32.to_be_bytes());
        descriptor[4..8].copy_from_slice(&0x8010_0000_u32.to_be_bytes());
        hpc1.load_scsi_descriptor(descriptor);
        hpc1.write(DeviceAddr::new(SCSI_CONTROL + 3), &[0x90])
            .unwrap();
        assert!(hpc1.consume_scsi_dma_bytes(8));
        hpc1.finish_scsi_dma();

        assert_eq!(read_word(&hpc1, SCSI_BYTE_COUNT), Ok(24));
        assert_eq!(
            read_word(&hpc1, SCSI_CURRENT_BUFFER_POINTER),
            Ok(0x0010_0008)
        );
        assert_eq!(read_word(&hpc1, SCSI_CONTROL), Ok(0x10));

        hpc1.write(DeviceAddr::new(SCSI_CONTROL + 3), &[0x90])
            .unwrap();
        assert!(hpc1.consume_scsi_dma_bytes(24));
        assert!(hpc1.scsi_dma_window().is_none());
        assert_eq!(read_word(&hpc1, SCSI_CONTROL), Ok(0x10));
    }

    #[test]
    fn descriptor_fields_apply_hardware_masks() {
        let mut hpc1 = Hpc1::new();
        let mut descriptor = [0; 12];
        descriptor[0..4].copy_from_slice(&u32::MAX.to_be_bytes());
        descriptor[4..8].copy_from_slice(&u32::MAX.to_be_bytes());
        descriptor[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
        hpc1.load_scsi_descriptor(descriptor);

        assert_eq!(read_word(&hpc1, SCSI_BYTE_COUNT), Ok(0x1fff));
        assert_eq!(
            read_word(&hpc1, SCSI_CURRENT_BUFFER_POINTER),
            Ok(0x0fff_ffff)
        );
        assert_eq!(
            read_word(&hpc1, SCSI_NEXT_DESCRIPTOR_POINTER),
            Ok(0x0fff_ffff)
        );
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
        hpc1.write(
            DeviceAddr::new(SCSI_NEXT_DESCRIPTOR_POINTER),
            &0x0012_3400_u32.to_be_bytes(),
        )
        .unwrap();
        hpc1.write(DeviceAddr::new(ENDIAN_CONTROL + 3), &[0x1f])
            .unwrap();
        hpc1.write(DeviceAddr::new(MISCELLANEOUS_CONTROL), &9_u32.to_be_bytes())
            .unwrap();
        hpc1.advance_time(VirtualDuration::from_attoseconds(
            ATTOSECONDS_PER_SECOND / FREE_RUNNING_COUNTER_FREQUENCY,
        ));

        hpc1.reset();

        assert_eq!(read_word(&hpc1, SCSI_CONTROL), Ok(0));
        assert_eq!(read_word(&hpc1, ENDIAN_CONTROL), Ok(0x40));
        assert_eq!(read_word(&hpc1, FREE_RUNNING_COUNTER), Ok(0));
        assert_eq!(read_word(&hpc1, MISCELLANEOUS_CONTROL), Ok(0));
        assert!(!hpc1.take_scsi_reset_request());
        assert!(hpc1.take_scsi_descriptor_fetch().is_none());
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
