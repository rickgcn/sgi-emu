//! Silicon Graphics PIC1 reset and graphics-DMA register front end.

use se_core::bus::{BusFault, DeviceAddr};

const CPU_CONTROL: u64 = 0x0000;
const RESET_CONFIGURATION: u64 = 0x0004;
const SYSTEM_ID: u64 = 0x0008;
const PARITY_ERROR: u64 = 0x1_0200;
const CLEAR_ERROR: u64 = 0x1_0210;
const DESCRIPTOR_ARRAY_BASE: u64 = 0xa_0000;

const REGISTER_BYTES: u64 = 4;
const SYSTEM_INITIALIZE: u16 = 1 << 9;
const DMA_IDLE: u16 = 1 << 3;
const FLOATING_POINT_ABSENT: u16 = 1;
const DESCRIPTOR_ADDRESS_MASK: u32 = 0x0fff_ffff;

/// The software-visible PIC1 state needed by the IP12 reset path.
pub struct Pic1 {
    reset_configuration: u8,
    revision: u8,
    floating_point_present: bool,
    cpu_control: u16,
    parity_error: u8,
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
            parity_error: 0,
            descriptor_array_base: 0,
            system_reset_requested: false,
        }
    }

    /// Restores the mutable PIC1 reset state.
    pub fn reset(&mut self) {
        self.cpu_control = 0;
        self.parity_error = 0;
        self.descriptor_array_base = 0;
        self.system_reset_requested = false;
    }

    /// Reads one fixed-width device-local transaction.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault`] when the complete transaction is not mapped or the
    /// requested width or direction is unsupported.
    pub fn read(&self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusFault> {
        let (start, end) = transaction_bounds(address, data.len())?;

        if let Some(offset) = register_offset(start, end, CPU_CONTROL) {
            read_register(u32::from(self.cpu_control), offset, data);
        } else if let Some(offset) = register_offset(start, end, RESET_CONFIGURATION) {
            read_register(u32::from(self.reset_configuration), offset, data);
        } else if let Some(offset) = register_offset(start, end, SYSTEM_ID) {
            read_register(u32::from(self.system_id()), offset, data);
        } else if let Some(offset) = register_offset(start, end, PARITY_ERROR) {
            read_register(u32::from(self.parity_error), offset, data);
        } else if register_offset(start, end, CLEAR_ERROR).is_some() {
            return Err(BusFault::UnsupportedAccess);
        } else if let Some(offset) = register_offset(start, end, DESCRIPTOR_ARRAY_BASE) {
            read_register(self.descriptor_array_base, offset, data);
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

        if let Some(offset) = register_offset(start, end, CPU_CONTROL) {
            let value = write_register(u32::from(self.cpu_control), offset, data) as u16;
            if value & SYSTEM_INITIALIZE != 0 {
                self.system_reset_requested = true;
            }
            self.cpu_control = value & !SYSTEM_INITIALIZE;
        } else if register_offset(start, end, RESET_CONFIGURATION).is_some()
            || register_offset(start, end, SYSTEM_ID).is_some()
            || register_offset(start, end, PARITY_ERROR).is_some()
        {
            return Err(BusFault::UnsupportedAccess);
        } else if register_offset(start, end, CLEAR_ERROR).is_some() {
            self.parity_error = 0;
        } else if let Some(offset) = register_offset(start, end, DESCRIPTOR_ARRAY_BASE) {
            self.descriptor_array_base =
                write_register(self.descriptor_array_base, offset, data) & DESCRIPTOR_ADDRESS_MASK;
        } else {
            return Err(BusFault::Unmapped);
        }

        Ok(())
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
    use se_core::bus::{BusFault, DeviceAddr};

    use super::{
        CLEAR_ERROR, CPU_CONTROL, DESCRIPTOR_ARRAY_BASE, PARITY_ERROR, Pic1, RESET_CONFIGURATION,
        SYSTEM_ID,
    };

    fn pic1() -> Pic1 {
        Pic1::new(0xf7, 2, true)
    }

    fn read_word(pic1: &Pic1, address: u64) -> Result<u32, BusFault> {
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
        assert_eq!(read_word(&pic1, PARITY_ERROR), Ok(0));
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
    fn clear_error_is_a_write_only_strobe() {
        let mut pic1 = pic1();
        pic1.parity_error = 0xa5;

        assert_eq!(
            pic1.read(DeviceAddr::new(CLEAR_ERROR), &mut [0; 1]),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(
            pic1.write(DeviceAddr::new(CLEAR_ERROR + 1), &[0x12, 0x34]),
            Ok(())
        );
        assert_eq!(read_word(&pic1, PARITY_ERROR), Ok(0));
    }

    #[test]
    fn read_only_registers_reject_writes() {
        let mut pic1 = pic1();

        for address in [RESET_CONFIGURATION, SYSTEM_ID, PARITY_ERROR] {
            assert_eq!(
                pic1.write(DeviceAddr::new(address), &[0]),
                Err(BusFault::UnsupportedAccess)
            );
        }
    }

    #[test]
    fn reset_clears_mutable_state_and_pending_requests() {
        let mut pic1 = pic1();
        pic1.parity_error = 0xff;
        pic1.write(DeviceAddr::new(CPU_CONTROL), &0x0000_0201_u32.to_be_bytes())
            .unwrap();
        pic1.write(
            DeviceAddr::new(DESCRIPTOR_ARRAY_BASE),
            &0x0123_4567_u32.to_be_bytes(),
        )
        .unwrap();

        pic1.reset();

        assert_eq!(read_word(&pic1, CPU_CONTROL), Ok(0));
        assert_eq!(read_word(&pic1, PARITY_ERROR), Ok(0));
        assert_eq!(read_word(&pic1, DESCRIPTOR_ARRAY_BASE), Ok(0));
        assert_eq!(read_word(&pic1, RESET_CONFIGURATION), Ok(0xf7));
        assert_eq!(read_word(&pic1, SYSTEM_ID), Ok(0x88));
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
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(
            pic1.write(DeviceAddr::new(DESCRIPTOR_ARRAY_BASE + 3), &[1, 2]),
            Err(BusFault::Unmapped)
        );
        assert_eq!(
            pic1.read(DeviceAddr::new(0x100), &mut [0; 1]),
            Err(BusFault::Unmapped)
        );
        assert_eq!(read_word(&pic1, DESCRIPTOR_ARRAY_BASE), Ok(0x0123_4567));
    }
}
