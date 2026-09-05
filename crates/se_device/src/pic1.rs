//! Silicon Graphics PIC1 reset, GIO configuration, and graphics-DMA register front end.

use se_core::bus::{BusError, DeviceAddr, PhysAddr};
use serde::{Deserialize, Serialize};

const CPU_CONTROL: u64 = 0x0000;
const RESET_CONFIGURATION: u64 = 0x0004;
const SYSTEM_ID: u64 = 0x0008;
const MEMORY_CONFIGURATION_0: u64 = 0x1_0000;
const MEMORY_CONFIGURATION_1: u64 = 0x1_0004;
const PARITY_ERROR: u64 = 0x1_0200;
const CPU_ERROR_ADDRESS: u64 = 0x1_0204;
const GIO_ERROR_ADDRESS: u64 = 0x1_0208;
const CLEAR_ERROR: u64 = 0x1_0210;
const GIO_SLOT_CONFIGURATION_0: u64 = 0x2_0000;
const GIO_SLOT_CONFIGURATION_1: u64 = 0x2_0004;
const GIO_BURST: u64 = 0x2_0008;
const GIO_DELAY: u64 = 0x2_000c;
const THREE_WAY_MASK: u64 = 0x8_0008;
const THREE_WAY_SUBSTITUTION: u64 = 0x8_000c;
const DESCRIPTOR_ARRAY_BASE: u64 = 0xa_0000;

const REGISTER_BYTES: u64 = 4;
const SYSTEM_INITIALIZE: u16 = 1 << 9;
const DMA_IDLE: u16 = 1 << 3;
const FLOATING_POINT_ABSENT: u16 = 1;
const MEMORY_DESCRIPTOR_MASK: u16 = 0x0f3f;
const GIO_SLOT_CONFIGURATION_MASK: u8 = 0x03;
const THREE_WAY_VALUE_MASK: u32 = 0x1fff_ffff;
const DESCRIPTOR_ADDRESS_MASK: u32 = 0x0fff_ffff;

/// The software-visible PIC1 state needed by the IP12 reset path.
#[derive(Clone, Deserialize, Serialize)]
pub struct Pic1 {
    reset_configuration: u8,
    revision: u8,
    floating_point_present: bool,
    cpu_control: u16,
    memory_descriptors: [u16; 4],
    parity_error: u8,
    cpu_error_address: u32,
    gio_error_address: u32,
    address_error_pending: bool,
    gio_slot_configurations: [u8; 2],
    gio_burst: u8,
    gio_delay: u8,
    three_way_mask: u32,
    three_way_substitution: u32,
    descriptor_array_base: u32,
    system_reset_requested: bool,
}

impl Pic1 {
    /// Creates a PIC1 with fixed board reset inputs.
    ///
    /// # Panics
    ///
    /// Panics when `revision` does not fit the three-bit SYSID revision field.
    #[must_use]
    pub const fn new(reset_configuration: u8, revision: u8, floating_point_present: bool) -> Self {
        assert!(revision <= 7, "PIC1 revision must fit in three bits");

        Self {
            reset_configuration,
            revision,
            floating_point_present,
            cpu_control: 0,
            memory_descriptors: [0; 4],
            parity_error: 0,
            cpu_error_address: 0,
            gio_error_address: 0,
            address_error_pending: false,
            gio_slot_configurations: [0; 2],
            gio_burst: 0,
            gio_delay: 0,
            three_way_mask: 0,
            three_way_substitution: 0,
            descriptor_array_base: 0,
            system_reset_requested: false,
        }
    }

    /// Restores the mutable PIC1 reset state.
    pub fn reset(&mut self) {
        self.cpu_control = 0;
        self.memory_descriptors = [0; 4];
        self.parity_error = 0;
        self.cpu_error_address = 0;
        self.gio_error_address = 0;
        self.address_error_pending = false;
        self.gio_slot_configurations = [0; 2];
        self.gio_burst = 0;
        self.gio_delay = 0;
        self.three_way_mask = 0;
        self.three_way_substitution = 0;
        self.descriptor_array_base = 0;
        self.system_reset_requested = false;
    }

    /// Reads one fixed-width device-local transaction.
    ///
    /// # Errors
    ///
    /// Returns [`BusError::InvalidTransaction`] for an invalid length or
    /// address overflow, [`BusError::HardwareFault`] for transactions crossing
    /// ordinary register boundaries, or [`BusError::UnimplementedAccess`] when
    /// the register, width, or direction is not implemented.
    pub fn read(&self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusError> {
        let (start, end) = transaction_bounds(address, data.len())?;

        if let Some(index) = memory_configuration_index(start, end)? {
            let value = u32::from(self.memory_descriptors[index]) << 16
                | u32::from(self.memory_descriptors[index + 1]);
            data.copy_from_slice(&value.to_be_bytes());
            return Ok(());
        }

        if let Some(offset) = register_offset(start, end, CPU_CONTROL) {
            read_register(u32::from(self.cpu_control), offset, data);
        } else if let Some(offset) = register_offset(start, end, RESET_CONFIGURATION) {
            read_register(u32::from(self.reset_configuration), offset, data);
        } else if let Some(offset) = register_offset(start, end, SYSTEM_ID) {
            read_register(u32::from(self.system_id()), offset, data);
        } else if let Some(offset) = register_offset(start, end, PARITY_ERROR) {
            read_register(u32::from(self.parity_error), offset, data);
        } else if let Some(offset) = register_offset(start, end, CPU_ERROR_ADDRESS) {
            read_register(self.cpu_error_address, offset, data);
        } else if let Some(offset) = register_offset(start, end, GIO_ERROR_ADDRESS) {
            read_register(self.gio_error_address, offset, data);
        } else if register_offset(start, end, CLEAR_ERROR).is_some() {
            return Err(BusError::UnimplementedAccess);
        } else if let Some(offset) = register_offset(start, end, GIO_SLOT_CONFIGURATION_0) {
            read_register(u32::from(self.gio_slot_configurations[0]), offset, data);
        } else if let Some(offset) = register_offset(start, end, GIO_SLOT_CONFIGURATION_1) {
            read_register(u32::from(self.gio_slot_configurations[1]), offset, data);
        } else if let Some(offset) = register_offset(start, end, GIO_BURST) {
            read_register(u32::from(self.gio_burst), offset, data);
        } else if let Some(offset) = register_offset(start, end, GIO_DELAY) {
            read_register(u32::from(self.gio_delay), offset, data);
        } else if let Some(offset) = register_offset(start, end, THREE_WAY_MASK) {
            read_register(self.three_way_mask, offset, data);
        } else if let Some(offset) = register_offset(start, end, THREE_WAY_SUBSTITUTION) {
            read_register(self.three_way_substitution, offset, data);
        } else if let Some(offset) = register_offset(start, end, DESCRIPTOR_ARRAY_BASE) {
            read_register(self.descriptor_array_base, offset, data);
        } else if start / REGISTER_BYTES != (end - 1) / REGISTER_BYTES {
            return Err(BusError::HardwareFault);
        } else {
            return Err(BusError::UnimplementedAccess);
        }

        Ok(())
    }

    /// Writes one fixed-width device-local transaction.
    ///
    /// # Errors
    ///
    /// Returns [`BusError::InvalidTransaction`] for an invalid length or
    /// address overflow, [`BusError::HardwareFault`] for transactions crossing
    /// ordinary register boundaries, or [`BusError::UnimplementedAccess`] when
    /// the register, width, or direction is not implemented.
    pub fn write(&mut self, address: DeviceAddr, data: &[u8]) -> Result<(), BusError> {
        let (start, end) = transaction_bounds(address, data.len())?;

        if let Some(index) = memory_configuration_index(start, end)? {
            let value =
                u32::from_be_bytes(data.try_into().map_err(|_| BusError::InvalidTransaction)?);
            self.memory_descriptors[index] = (value >> 16) as u16 & MEMORY_DESCRIPTOR_MASK;
            self.memory_descriptors[index + 1] = value as u16 & MEMORY_DESCRIPTOR_MASK;
            return Ok(());
        }

        if let Some(offset) = register_offset(start, end, CPU_CONTROL) {
            let value = write_register(u32::from(self.cpu_control), offset, data) as u16;
            if value & SYSTEM_INITIALIZE != 0 {
                self.system_reset_requested = true;
            }
            self.cpu_control = value & !SYSTEM_INITIALIZE;
        } else if register_offset(start, end, RESET_CONFIGURATION).is_some()
            || register_offset(start, end, SYSTEM_ID).is_some()
            || register_offset(start, end, PARITY_ERROR).is_some()
            || register_offset(start, end, CPU_ERROR_ADDRESS).is_some()
            || register_offset(start, end, GIO_ERROR_ADDRESS).is_some()
        {
            return Err(BusError::UnimplementedAccess);
        } else if register_offset(start, end, CLEAR_ERROR).is_some() {
            self.parity_error = 0;
            self.address_error_pending = false;
        } else if let Some(offset) = register_offset(start, end, GIO_SLOT_CONFIGURATION_0) {
            self.gio_slot_configurations[0] =
                write_register(u32::from(self.gio_slot_configurations[0]), offset, data) as u8
                    & GIO_SLOT_CONFIGURATION_MASK;
        } else if let Some(offset) = register_offset(start, end, GIO_SLOT_CONFIGURATION_1) {
            self.gio_slot_configurations[1] =
                write_register(u32::from(self.gio_slot_configurations[1]), offset, data) as u8
                    & GIO_SLOT_CONFIGURATION_MASK;
        } else if let Some(offset) = register_offset(start, end, GIO_BURST) {
            self.gio_burst = write_register(u32::from(self.gio_burst), offset, data) as u8;
        } else if let Some(offset) = register_offset(start, end, GIO_DELAY) {
            self.gio_delay = write_register(u32::from(self.gio_delay), offset, data) as u8;
        } else if let Some(offset) = register_offset(start, end, THREE_WAY_MASK) {
            self.three_way_mask =
                write_register(self.three_way_mask, offset, data) & THREE_WAY_VALUE_MASK;
        } else if let Some(offset) = register_offset(start, end, THREE_WAY_SUBSTITUTION) {
            self.three_way_substitution =
                write_register(self.three_way_substitution, offset, data) & THREE_WAY_VALUE_MASK;
        } else if let Some(offset) = register_offset(start, end, DESCRIPTOR_ARRAY_BASE) {
            self.descriptor_array_base =
                write_register(self.descriptor_array_base, offset, data) & DESCRIPTOR_ADDRESS_MASK;
        } else if start / REGISTER_BYTES != (end - 1) / REGISTER_BYTES {
            return Err(BusError::HardwareFault);
        } else {
            return Err(BusError::UnimplementedAccess);
        }

        Ok(())
    }

    /// Decodes one physical transaction through the memory configuration.
    ///
    /// Returns the matching descriptor index and descriptor-relative byte
    /// address. Installed storage is a property of the containing machine and
    /// is not considered by this method.
    ///
    /// # Errors
    ///
    /// Returns [`BusError::InvalidTransaction`] when `byte_len` is zero or
    /// the address range overflows.
    pub fn decode_memory(
        &self,
        address: PhysAddr,
        byte_len: usize,
    ) -> Result<Option<(usize, DeviceAddr)>, BusError> {
        if byte_len == 0 {
            return Err(BusError::InvalidTransaction);
        }

        let start = address.get();
        let length = u64::try_from(byte_len).map_err(|_| BusError::InvalidTransaction)?;
        let end = start
            .checked_add(length)
            .ok_or(BusError::InvalidTransaction)?;

        for (index, descriptor) in self.memory_descriptors.iter().copied().enumerate() {
            let Some(size) = memory_size(descriptor) else {
                continue;
            };
            let base = u64::from(descriptor & 0x003f) << 22;
            let window_end = base + size;
            if start >= base && end <= window_end {
                return Ok(Some((index, DeviceAddr::new(start - base))));
            }
        }

        Ok(None)
    }

    /// Records an asynchronous address error.
    pub fn report_address_error(&mut self) {
        self.address_error_pending = true;
    }

    /// Returns whether the addressing and parity error output is asserted.
    #[must_use]
    pub fn error_interrupt_asserted(&self) -> bool {
        self.address_error_pending || self.parity_error != 0
    }

    /// Returns and clears a pending whole-system reset request.
    pub fn take_system_reset_request(&mut self) -> bool {
        let requested = self.system_reset_requested;
        self.system_reset_requested = false;
        requested
    }

    const fn system_id(&self) -> u16 {
        let floating_point = if self.floating_point_present {
            0
        } else {
            FLOATING_POINT_ABSENT
        };
        (self.revision as u16) << 6 | DMA_IDLE | floating_point
    }
}

fn transaction_bounds(address: DeviceAddr, length: usize) -> Result<(u64, u64), BusError> {
    if !(1..=4).contains(&length) {
        return Err(BusError::InvalidTransaction);
    }

    let start = address.get();
    let length = u64::try_from(length).map_err(|_| BusError::InvalidTransaction)?;
    let end = start
        .checked_add(length)
        .ok_or(BusError::InvalidTransaction)?;
    Ok((start, end))
}

fn memory_configuration_index(start: u64, end: u64) -> Result<Option<usize>, BusError> {
    for (index, base) in [MEMORY_CONFIGURATION_0, MEMORY_CONFIGURATION_1]
        .into_iter()
        .enumerate()
    {
        let register_end = base + REGISTER_BYTES;
        if start == base && end == register_end {
            return Ok(Some(index * 2));
        }
        if start < register_end && end > base {
            return Err(BusError::UnimplementedAccess);
        }
    }

    Ok(None)
}

const fn memory_size(descriptor: u16) -> Option<u64> {
    match (descriptor >> 8) & 0x000f {
        0x0 => Some(4 * 1024 * 1024),
        0x1 => Some(8 * 1024 * 1024),
        0x3 => Some(16 * 1024 * 1024),
        0x7 => Some(32 * 1024 * 1024),
        0xf => Some(64 * 1024 * 1024),
        _ => None,
    }
}

fn register_offset(start: u64, end: u64, register: u64) -> Option<usize> {
    if start < register || end > register + REGISTER_BYTES {
        return None;
    }

    usize::try_from(start - register).ok()
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
    use se_core::bus::{BusError, DeviceAddr, PhysAddr};

    use super::{
        CLEAR_ERROR, CPU_CONTROL, CPU_ERROR_ADDRESS, DESCRIPTOR_ARRAY_BASE, GIO_BURST, GIO_DELAY,
        GIO_ERROR_ADDRESS, GIO_SLOT_CONFIGURATION_0, GIO_SLOT_CONFIGURATION_1,
        MEMORY_CONFIGURATION_0, MEMORY_CONFIGURATION_1, PARITY_ERROR, Pic1, RESET_CONFIGURATION,
        SYSTEM_ID, THREE_WAY_MASK, THREE_WAY_SUBSTITUTION,
    };

    fn pic1() -> Pic1 {
        Pic1::new(0xf7, 2, true)
    }

    fn read_word(pic1: &Pic1, address: u64) -> Result<u32, BusError> {
        let mut bytes = [0; 4];
        pic1.read(DeviceAddr::new(address), &mut bytes)?;
        Ok(u32::from_be_bytes(bytes))
    }

    #[test]
    fn reset_values_match_the_ip12_profile() {
        let pic1 = pic1();

        assert_eq!(read_word(&pic1, CPU_CONTROL), Ok(0));
        assert_eq!(read_word(&pic1, RESET_CONFIGURATION), Ok(0xf7));
        assert_eq!(read_word(&pic1, SYSTEM_ID), Ok(0x88));
        assert_eq!(read_word(&pic1, MEMORY_CONFIGURATION_0), Ok(0));
        assert_eq!(read_word(&pic1, MEMORY_CONFIGURATION_1), Ok(0));
        assert_eq!(read_word(&pic1, PARITY_ERROR), Ok(0));
        assert_eq!(read_word(&pic1, CPU_ERROR_ADDRESS), Ok(0));
        assert_eq!(read_word(&pic1, GIO_ERROR_ADDRESS), Ok(0));
        assert_eq!(read_word(&pic1, GIO_SLOT_CONFIGURATION_0), Ok(0));
        assert_eq!(read_word(&pic1, GIO_SLOT_CONFIGURATION_1), Ok(0));
        assert_eq!(read_word(&pic1, GIO_BURST), Ok(0));
        assert_eq!(read_word(&pic1, GIO_DELAY), Ok(0));
        assert_eq!(read_word(&pic1, THREE_WAY_MASK), Ok(0));
        assert_eq!(read_word(&pic1, THREE_WAY_SUBSTITUTION), Ok(0));
        assert_eq!(read_word(&pic1, DESCRIPTOR_ARRAY_BASE), Ok(0));
    }

    #[test]
    fn system_id_marks_an_absent_floating_point_coprocessor() {
        let pic1 = Pic1::new(0xf7, 2, false);

        assert_eq!(read_word(&pic1, SYSTEM_ID), Ok(0x89));
    }

    #[test]
    #[should_panic(expected = "PIC1 revision must fit in three bits")]
    fn constructor_rejects_an_out_of_range_revision() {
        let _ = Pic1::new(0xf7, 8, true);
    }

    #[test]
    fn cpu_control_stores_bits_and_turns_system_initialize_into_a_request() {
        let mut pic1 = pic1();

        assert_eq!(
            pic1.write(DeviceAddr::new(CPU_CONTROL), &0x0000_0e01_u32.to_be_bytes()),
            Ok(())
        );
        assert_eq!(read_word(&pic1, CPU_CONTROL), Ok(0x0000_0c01));
        assert!(pic1.take_system_reset_request());
        assert!(!pic1.take_system_reset_request());
    }

    #[test]
    fn cpu_control_uses_big_endian_lanes_and_ignores_high_word_lanes() {
        let mut pic1 = pic1();

        assert_eq!(
            pic1.write(DeviceAddr::new(CPU_CONTROL + 3), &[0x5a]),
            Ok(())
        );
        assert_eq!(pic1.write(DeviceAddr::new(CPU_CONTROL), &[0xff]), Ok(()));
        assert_eq!(read_word(&pic1, CPU_CONTROL), Ok(0x5a));
    }

    #[test]
    fn descriptor_array_base_uses_big_endian_lanes() {
        let mut pic1 = pic1();

        assert_eq!(
            pic1.write(
                DeviceAddr::new(DESCRIPTOR_ARRAY_BASE),
                &0x0000_000f_u32.to_be_bytes()
            ),
            Ok(())
        );

        let mut first = [0xff];
        let mut last = [0];
        assert_eq!(
            pic1.read(DeviceAddr::new(DESCRIPTOR_ARRAY_BASE), &mut first),
            Ok(())
        );
        assert_eq!(
            pic1.read(DeviceAddr::new(DESCRIPTOR_ARRAY_BASE + 3), &mut last),
            Ok(())
        );
        assert_eq!(first, [0]);
        assert_eq!(last, [0x0f]);
    }

    #[test]
    fn descriptor_array_base_masks_undefined_high_bits() {
        let mut pic1 = pic1();

        assert_eq!(
            pic1.write(DeviceAddr::new(DESCRIPTOR_ARRAY_BASE), &[0xff; 4]),
            Ok(())
        );
        assert_eq!(read_word(&pic1, DESCRIPTOR_ARRAY_BASE), Ok(0x0fff_ffff));
    }

    #[test]
    fn gio_registers_use_independent_low_big_endian_lanes() {
        let mut pic1 = pic1();

        assert_eq!(
            pic1.write(DeviceAddr::new(GIO_BURST), &0xff00_0001_u32.to_be_bytes()),
            Ok(())
        );
        assert_eq!(pic1.write(DeviceAddr::new(GIO_DELAY + 3), &[0xf2]), Ok(()));
        assert_eq!(pic1.write(DeviceAddr::new(GIO_DELAY), &[0xff]), Ok(()));

        assert_eq!(read_word(&pic1, GIO_BURST), Ok(1));
        assert_eq!(read_word(&pic1, GIO_DELAY), Ok(0xf2));
    }

    #[test]
    fn gio_slot_configurations_store_two_bits_independently() {
        let mut pic1 = pic1();

        assert_eq!(
            pic1.write(
                DeviceAddr::new(GIO_SLOT_CONFIGURATION_0),
                &0xffff_ffff_u32.to_be_bytes()
            ),
            Ok(())
        );
        assert_eq!(
            pic1.write(DeviceAddr::new(GIO_SLOT_CONFIGURATION_1 + 3), &[0x02]),
            Ok(())
        );

        assert_eq!(read_word(&pic1, GIO_SLOT_CONFIGURATION_0), Ok(0x03));
        assert_eq!(read_word(&pic1, GIO_SLOT_CONFIGURATION_1), Ok(0x02));
    }

    #[test]
    fn three_way_address_registers_store_29_bits_independently() {
        let mut pic1 = pic1();

        assert_eq!(
            pic1.write(DeviceAddr::new(THREE_WAY_MASK), &[0xff; 4]),
            Ok(())
        );
        assert_eq!(
            pic1.write(
                DeviceAddr::new(THREE_WAY_SUBSTITUTION),
                &0x1234_5678_u32.to_be_bytes()
            ),
            Ok(())
        );

        assert_eq!(read_word(&pic1, THREE_WAY_MASK), Ok(0x1fff_ffff));
        assert_eq!(read_word(&pic1, THREE_WAY_SUBSTITUTION), Ok(0x1234_5678));
    }

    #[test]
    fn ide_graphics_dma_channel_register_sequence_is_mapped() {
        const PATTERNS: [u32; 12] = [
            0xaaaa_aaaa,
            0x5555_5555,
            0xcccc_cccc,
            0x3333_3333,
            0xf0f0_f0f0,
            0x0f0f_0f0f,
            0xff00_ff00,
            0x00ff_00ff,
            0xffff_0000,
            0x0000_ffff,
            0xffff_ffff,
            0x0000_0000,
        ];

        let mut pic1 = pic1();
        for (address, mask, pattern_count) in [
            (DESCRIPTOR_ARRAY_BASE, 0x0fff_ffff, 12),
            (THREE_WAY_MASK, 0x0fff_ffff, 12),
            (THREE_WAY_SUBSTITUTION, 0x0fff_ffff, 12),
            (GIO_DELAY, 0x0000_00ff, 8),
            (GIO_BURST, 0x0000_00ff, 8),
            (GIO_SLOT_CONFIGURATION_1, 0x0000_0003, 4),
            (GIO_SLOT_CONFIGURATION_0, 0x0000_0003, 4),
        ] {
            for pattern in PATTERNS.into_iter().take(pattern_count) {
                let expected = pattern & mask;
                assert_eq!(
                    pic1.write(DeviceAddr::new(address), &expected.to_be_bytes()),
                    Ok(())
                );
                assert_eq!(read_word(&pic1, address), Ok(expected));
            }
        }
    }

    #[test]
    fn clear_error_is_a_write_only_strobe() {
        let mut pic1 = pic1();
        pic1.parity_error = 0xa5;
        pic1.cpu_error_address = 0x1234_5678;
        pic1.gio_error_address = 0x9abc_def0;
        pic1.report_address_error();

        assert!(pic1.error_interrupt_asserted());

        assert_eq!(
            pic1.read(DeviceAddr::new(CLEAR_ERROR), &mut [0; 1]),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(
            pic1.write(DeviceAddr::new(CLEAR_ERROR + 1), &[0x12, 0x34]),
            Ok(())
        );
        assert_eq!(read_word(&pic1, PARITY_ERROR), Ok(0));
        assert_eq!(read_word(&pic1, CPU_ERROR_ADDRESS), Ok(0x1234_5678));
        assert_eq!(read_word(&pic1, GIO_ERROR_ADDRESS), Ok(0x9abc_def0));
        assert!(!pic1.error_interrupt_asserted());
    }

    #[test]
    fn error_address_registers_use_big_endian_lanes() {
        let mut pic1 = pic1();
        pic1.cpu_error_address = 0x1234_5678;
        pic1.gio_error_address = 0x9abc_def0;

        assert_eq!(read_word(&pic1, CPU_ERROR_ADDRESS), Ok(0x1234_5678));
        assert_eq!(read_word(&pic1, GIO_ERROR_ADDRESS), Ok(0x9abc_def0));

        let mut cpu_first = [0];
        let mut cpu_last = [0];
        let mut gio_middle = [0; 2];
        assert_eq!(
            pic1.read(DeviceAddr::new(CPU_ERROR_ADDRESS), &mut cpu_first),
            Ok(())
        );
        assert_eq!(
            pic1.read(DeviceAddr::new(CPU_ERROR_ADDRESS + 3), &mut cpu_last),
            Ok(())
        );
        assert_eq!(
            pic1.read(DeviceAddr::new(GIO_ERROR_ADDRESS + 1), &mut gio_middle),
            Ok(())
        );
        assert_eq!(cpu_first, [0x12]);
        assert_eq!(cpu_last, [0x78]);
        assert_eq!(gio_middle, [0xbc, 0xde]);
    }

    #[test]
    fn prom_error_register_sequence_is_mapped() {
        let mut pic1 = pic1();

        assert_eq!(read_word(&pic1, PARITY_ERROR), Ok(0));
        assert_eq!(read_word(&pic1, CPU_ERROR_ADDRESS), Ok(0));
        assert_eq!(read_word(&pic1, GIO_ERROR_ADDRESS), Ok(0));
        assert_eq!(
            pic1.write(DeviceAddr::new(CLEAR_ERROR), &0_u32.to_be_bytes()),
            Ok(())
        );
    }

    #[test]
    fn memory_configuration_requires_aligned_words_and_masks_reserved_bits() {
        let mut pic1 = pic1();

        assert_eq!(
            pic1.write(
                DeviceAddr::new(MEMORY_CONFIGURATION_0),
                &0xffff_ffff_u32.to_be_bytes()
            ),
            Ok(())
        );
        assert_eq!(read_word(&pic1, MEMORY_CONFIGURATION_0), Ok(0x0f3f_0f3f));
        assert_eq!(
            pic1.read(DeviceAddr::new(MEMORY_CONFIGURATION_0), &mut [0]),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(
            pic1.write(DeviceAddr::new(MEMORY_CONFIGURATION_0 + 1), &[0; 4]),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(
            pic1.write(DeviceAddr::new(MEMORY_CONFIGURATION_1 - 1), &[0; 2]),
            Err(BusError::UnimplementedAccess)
        );
    }

    #[test]
    fn memory_decoder_supports_every_documented_size() {
        let mut pic1 = pic1();

        for (size_code, byte_len) in [
            (0x0_u16, 4_u64 * 1024 * 1024),
            (0x1, 8 * 1024 * 1024),
            (0x3, 16 * 1024 * 1024),
            (0x7, 32 * 1024 * 1024),
            (0xf, 64 * 1024 * 1024),
        ] {
            let descriptor = size_code << 8 | 5;
            let value = u32::from(descriptor) << 16 | 0x023f;
            pic1.write(
                DeviceAddr::new(MEMORY_CONFIGURATION_0),
                &value.to_be_bytes(),
            )
            .unwrap();

            let base = 5_u64 << 22;
            assert_eq!(
                pic1.decode_memory(PhysAddr::new(base), 4),
                Ok(Some((0, DeviceAddr::new(0))))
            );
            assert_eq!(
                pic1.decode_memory(PhysAddr::new(base + byte_len - 4), 4),
                Ok(Some((0, DeviceAddr::new(byte_len - 4))))
            );
            assert_eq!(
                pic1.decode_memory(PhysAddr::new(base + byte_len), 1),
                Ok(None)
            );
        }
    }

    #[test]
    fn memory_decoder_rejects_undefined_sizes_and_crossing_transactions() {
        let mut pic1 = pic1();
        let base = 5_u64 << 22;

        for size_code in [0x2_u16, 0x4, 0x5, 0x6, 0x8, 0x9, 0xa, 0xb, 0xc, 0xd, 0xe] {
            let descriptor = size_code << 8 | 5;
            let value = u32::from(descriptor) << 16 | 0x023f;
            pic1.write(
                DeviceAddr::new(MEMORY_CONFIGURATION_0),
                &value.to_be_bytes(),
            )
            .unwrap();

            assert_eq!(pic1.decode_memory(PhysAddr::new(base), 4), Ok(None));
        }

        pic1.write(
            DeviceAddr::new(MEMORY_CONFIGURATION_0),
            &(u32::from(5_u16) << 16 | 0x023f).to_be_bytes(),
        )
        .unwrap();
        assert_eq!(
            pic1.decode_memory(PhysAddr::new(base + 4 * 1024 * 1024 - 2), 4),
            Ok(None)
        );
        assert_eq!(
            pic1.decode_memory(PhysAddr::new(base), 0),
            Err(BusError::InvalidTransaction)
        );
        assert_eq!(
            pic1.decode_memory(PhysAddr::new(base), 5),
            Ok(Some((0, DeviceAddr::new(0))))
        );
    }

    #[test]
    fn write_bus_errors_latch_until_clear_or_reset() {
        let mut pic1 = pic1();

        assert!(!pic1.error_interrupt_asserted());
        pic1.report_address_error();
        assert!(pic1.error_interrupt_asserted());

        pic1.reset();
        assert!(!pic1.error_interrupt_asserted());
    }

    #[test]
    fn read_only_registers_reject_writes() {
        let mut pic1 = pic1();

        for address in [
            RESET_CONFIGURATION,
            SYSTEM_ID,
            PARITY_ERROR,
            CPU_ERROR_ADDRESS,
            GIO_ERROR_ADDRESS,
        ] {
            assert_eq!(
                pic1.write(DeviceAddr::new(address), &[0]),
                Err(BusError::UnimplementedAccess)
            );
        }
    }

    #[test]
    fn reset_clears_mutable_state_and_pending_requests() {
        let mut pic1 = pic1();
        pic1.parity_error = 0xff;
        pic1.cpu_error_address = 0x1234_5678;
        pic1.gio_error_address = 0x9abc_def0;
        pic1.report_address_error();
        pic1.write(DeviceAddr::new(CPU_CONTROL), &0x0000_0201_u32.to_be_bytes())
            .unwrap();
        pic1.write(
            DeviceAddr::new(MEMORY_CONFIGURATION_0),
            &0x0100_003f_u32.to_be_bytes(),
        )
        .unwrap();
        pic1.write(
            DeviceAddr::new(DESCRIPTOR_ARRAY_BASE),
            &0x0123_4567_u32.to_be_bytes(),
        )
        .unwrap();
        pic1.write(DeviceAddr::new(GIO_BURST + 3), &[1]).unwrap();
        pic1.write(DeviceAddr::new(GIO_DELAY + 3), &[0xf2]).unwrap();
        pic1.write(DeviceAddr::new(GIO_SLOT_CONFIGURATION_0 + 3), &[3])
            .unwrap();
        pic1.write(DeviceAddr::new(GIO_SLOT_CONFIGURATION_1 + 3), &[2])
            .unwrap();
        pic1.write(DeviceAddr::new(THREE_WAY_MASK), &[0xff; 4])
            .unwrap();
        pic1.write(
            DeviceAddr::new(THREE_WAY_SUBSTITUTION),
            &0x1234_5678_u32.to_be_bytes(),
        )
        .unwrap();

        pic1.reset();

        assert_eq!(read_word(&pic1, CPU_CONTROL), Ok(0));
        assert_eq!(read_word(&pic1, MEMORY_CONFIGURATION_0), Ok(0));
        assert_eq!(read_word(&pic1, MEMORY_CONFIGURATION_1), Ok(0));
        assert_eq!(read_word(&pic1, PARITY_ERROR), Ok(0));
        assert_eq!(read_word(&pic1, CPU_ERROR_ADDRESS), Ok(0));
        assert_eq!(read_word(&pic1, GIO_ERROR_ADDRESS), Ok(0));
        assert_eq!(read_word(&pic1, GIO_SLOT_CONFIGURATION_0), Ok(0));
        assert_eq!(read_word(&pic1, GIO_SLOT_CONFIGURATION_1), Ok(0));
        assert_eq!(read_word(&pic1, GIO_BURST), Ok(0));
        assert_eq!(read_word(&pic1, GIO_DELAY), Ok(0));
        assert_eq!(read_word(&pic1, THREE_WAY_MASK), Ok(0));
        assert_eq!(read_word(&pic1, THREE_WAY_SUBSTITUTION), Ok(0));
        assert_eq!(read_word(&pic1, DESCRIPTOR_ARRAY_BASE), Ok(0));
        assert_eq!(read_word(&pic1, RESET_CONFIGURATION), Ok(0xf7));
        assert_eq!(read_word(&pic1, SYSTEM_ID), Ok(0x88));
        assert!(!pic1.error_interrupt_asserted());
        assert!(!pic1.take_system_reset_request());
    }

    #[test]
    fn rejects_invalid_unmapped_and_crossing_transactions_atomically() {
        let mut pic1 = pic1();
        pic1.write(
            DeviceAddr::new(DESCRIPTOR_ARRAY_BASE),
            &0x0123_4567_u32.to_be_bytes(),
        )
        .unwrap();

        assert_eq!(
            pic1.write(DeviceAddr::new(DESCRIPTOR_ARRAY_BASE), &[]),
            Err(BusError::InvalidTransaction)
        );
        assert_eq!(
            pic1.write(DeviceAddr::new(DESCRIPTOR_ARRAY_BASE + 3), &[1, 2]),
            Err(BusError::HardwareFault)
        );
        assert_eq!(
            pic1.read(DeviceAddr::new(CPU_ERROR_ADDRESS + 3), &mut [0; 2]),
            Err(BusError::HardwareFault)
        );
        assert_eq!(
            pic1.read(DeviceAddr::new(0x100), &mut [0; 1]),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(read_word(&pic1, DESCRIPTOR_ARRAY_BASE), Ok(0x0123_4567));
    }
}
