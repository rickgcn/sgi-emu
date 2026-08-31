//! Silicon Graphics INT2 interrupt and timer register front end.

use se_core::bus::{BusFault, DeviceAddr};

const LOCAL_INTERRUPT_0_STATUS: u64 = 0x00;
const LOCAL_INTERRUPT_0_MASK: u64 = 0x04;
const LOCAL_INTERRUPT_1_STATUS: u64 = 0x08;
const LOCAL_INTERRUPT_1_MASK: u64 = 0x0c;
const VME_INTERRUPT_STATUS: u64 = 0x10;
const VME_INTERRUPT_0_MASK: u64 = 0x14;
const VME_INTERRUPT_1_MASK: u64 = 0x18;
const OUTPUT_PORT: u64 = 0x1c;
const TIMER_ACKNOWLEDGE: u64 = 0x23;
const PROGRAMMABLE_TIMER_CLOCK: u64 = 0x30;
const REGISTER_BYTES: u64 = 4;
const OUTPUT_BITS: u8 = 0x1f;

/// The software-visible INT2 state used by the IP12 machine.
pub struct Int2 {
    local_interrupt_masks: [u8; 2],
    vme_interrupt_masks: [u8; 2],
    output_port: u8,
    timer_pending: [bool; 2],
    timer_programming: [u8; 3],
    timer_programming_length: usize,
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
            vme_interrupt_masks: [0; 2],
            output_port: 0,
            timer_pending: [false; 2],
            timer_programming: [0; 3],
            timer_programming_length: 0,
        }
    }

    /// Restores the mutable INT2 reset state.
    pub fn reset(&mut self) {
        self.local_interrupt_masks = [0; 2];
        self.vme_interrupt_masks = [0; 2];
        self.output_port = 0;
        self.timer_pending = [false; 2];
        self.timer_programming = [0; 3];
        self.timer_programming_length = 0;
    }

    /// Reads one fixed-width device-local transaction.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault`] when the complete transaction is not mapped or the
    /// requested width or direction is unsupported.
    pub fn read(&self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusFault> {
        let (start, end) = transaction_bounds(address, data.len())?;

        if let Some((register, offset)) = decode_word_register(start, end) {
            let value = match register {
                Register::LocalInterrupt0Status
                | Register::LocalInterrupt1Status
                | Register::VmeInterruptStatus => 0,
                Register::LocalInterrupt0Mask => self.local_interrupt_masks[0],
                Register::LocalInterrupt1Mask => self.local_interrupt_masks[1],
                Register::VmeInterrupt0Mask => self.vme_interrupt_masks[0],
                Register::VmeInterrupt1Mask => self.vme_interrupt_masks[1],
                Register::OutputPort => self.output_port,
            };
            read_register(u32::from(value), offset, data);
            return Ok(());
        }

        if start == TIMER_ACKNOWLEDGE || start == PROGRAMMABLE_TIMER_CLOCK {
            return Err(BusFault::UnsupportedAccess);
        }

        Err(BusFault::Unmapped)
    }

    /// Writes one fixed-width device-local transaction.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault`] when the complete transaction is not mapped or the
    /// requested width or direction is unsupported.
    pub fn write(&mut self, address: DeviceAddr, data: &[u8]) -> Result<(), BusFault> {
        let (start, end) = transaction_bounds(address, data.len())?;

        if let Some((register, offset)) = decode_word_register(start, end) {
            match register {
                Register::LocalInterrupt0Status
                | Register::LocalInterrupt1Status
                | Register::VmeInterruptStatus => return Err(BusFault::UnsupportedAccess),
                Register::LocalInterrupt0Mask => {
                    write_byte_register(&mut self.local_interrupt_masks[0], offset, data)
                }
                Register::LocalInterrupt1Mask => {
                    write_byte_register(&mut self.local_interrupt_masks[1], offset, data)
                }
                Register::VmeInterrupt0Mask => {
                    write_byte_register(&mut self.vme_interrupt_masks[0], offset, data)
                }
                Register::VmeInterrupt1Mask => {
                    write_byte_register(&mut self.vme_interrupt_masks[1], offset, data)
                }
                Register::OutputPort => {
                    write_byte_register(&mut self.output_port, offset, data);
                    self.output_port &= OUTPUT_BITS;
                }
            }
            return Ok(());
        }

        if start == TIMER_ACKNOWLEDGE && end == start + 1 {
            if data[0] & 1 != 0 {
                self.timer_pending[0] = false;
            }
            if data[0] & 2 != 0 {
                self.timer_pending[1] = false;
            }
            return Ok(());
        }

        if start == PROGRAMMABLE_TIMER_CLOCK && end == start + 1 {
            self.timer_programming.rotate_left(1);
            self.timer_programming[2] = data[0];
            self.timer_programming_length = (self.timer_programming_length + 1).min(3);
            return Ok(());
        }

        if start == TIMER_ACKNOWLEDGE || start == PROGRAMMABLE_TIMER_CLOCK {
            return Err(BusFault::UnsupportedAccess);
        }

        Err(BusFault::Unmapped)
    }
}

#[derive(Clone, Copy)]
enum Register {
    LocalInterrupt0Status,
    LocalInterrupt0Mask,
    LocalInterrupt1Status,
    LocalInterrupt1Mask,
    VmeInterruptStatus,
    VmeInterrupt0Mask,
    VmeInterrupt1Mask,
    OutputPort,
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

fn decode_word_register(start: u64, end: u64) -> Option<(Register, usize)> {
    for (base, register) in [
        (LOCAL_INTERRUPT_0_STATUS, Register::LocalInterrupt0Status),
        (LOCAL_INTERRUPT_0_MASK, Register::LocalInterrupt0Mask),
        (LOCAL_INTERRUPT_1_STATUS, Register::LocalInterrupt1Status),
        (LOCAL_INTERRUPT_1_MASK, Register::LocalInterrupt1Mask),
        (VME_INTERRUPT_STATUS, Register::VmeInterruptStatus),
        (VME_INTERRUPT_0_MASK, Register::VmeInterrupt0Mask),
        (VME_INTERRUPT_1_MASK, Register::VmeInterrupt1Mask),
        (OUTPUT_PORT, Register::OutputPort),
    ] {
        if start >= base && end <= base + REGISTER_BYTES {
            return usize::try_from(start - base)
                .ok()
                .map(|offset| (register, offset));
        }
    }
    None
}

fn read_register(value: u32, offset: usize, data: &mut [u8]) {
    data.copy_from_slice(&value.to_be_bytes()[offset..offset + data.len()]);
}

fn write_byte_register(register: &mut u8, offset: usize, data: &[u8]) {
    let mut bytes = u32::from(*register).to_be_bytes();
    bytes[offset..offset + data.len()].copy_from_slice(data);
    *register = bytes[3];
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusFault, DeviceAddr};

    use super::{
        Int2, LOCAL_INTERRUPT_0_MASK, LOCAL_INTERRUPT_0_STATUS, LOCAL_INTERRUPT_1_MASK,
        LOCAL_INTERRUPT_1_STATUS, OUTPUT_PORT, PROGRAMMABLE_TIMER_CLOCK, TIMER_ACKNOWLEDGE,
        VME_INTERRUPT_0_MASK, VME_INTERRUPT_1_MASK, VME_INTERRUPT_STATUS,
    };

    fn read_word(int2: &Int2, address: u64) -> Result<u32, BusFault> {
        let mut bytes = [0; 4];
        int2.read(DeviceAddr::new(address), &mut bytes)?;
        Ok(u32::from_be_bytes(bytes))
    }

    #[test]
    fn reset_values_are_inactive() {
        let int2 = Int2::new();

        for address in [
            LOCAL_INTERRUPT_0_STATUS,
            LOCAL_INTERRUPT_0_MASK,
            LOCAL_INTERRUPT_1_STATUS,
            LOCAL_INTERRUPT_1_MASK,
            VME_INTERRUPT_STATUS,
            VME_INTERRUPT_0_MASK,
            VME_INTERRUPT_1_MASK,
            OUTPUT_PORT,
        ] {
            assert_eq!(read_word(&int2, address), Ok(0));
        }
        assert_eq!(int2.timer_pending, [false; 2]);
    }

    #[test]
    fn masks_use_independent_low_big_endian_lanes() {
        let mut int2 = Int2::new();

        for (address, value) in [
            (LOCAL_INTERRUPT_0_MASK, 0xa5),
            (LOCAL_INTERRUPT_1_MASK, 0x5a),
            (VME_INTERRUPT_0_MASK, 0x3c),
            (VME_INTERRUPT_1_MASK, 0xc3),
        ] {
            int2.write(DeviceAddr::new(address + 3), &[value]).unwrap();
            assert_eq!(read_word(&int2, address), Ok(u32::from(value)));
        }
    }

    #[test]
    fn status_registers_are_read_only_and_inactive() {
        let mut int2 = Int2::new();

        for address in [
            LOCAL_INTERRUPT_0_STATUS,
            LOCAL_INTERRUPT_1_STATUS,
            VME_INTERRUPT_STATUS,
        ] {
            assert_eq!(
                int2.write(DeviceAddr::new(address), &[0; 4]),
                Err(BusFault::UnsupportedAccess)
            );
            assert_eq!(read_word(&int2, address), Ok(0));
        }
    }

    #[test]
    fn output_port_keeps_only_the_low_five_bits() {
        let mut int2 = Int2::new();

        int2.write(DeviceAddr::new(OUTPUT_PORT + 3), &[0xf6])
            .unwrap();

        assert_eq!(read_word(&int2, OUTPUT_PORT), Ok(0x16));
    }

    #[test]
    fn timer_acknowledge_clears_selected_pending_flags() {
        let mut int2 = Int2::new();
        int2.timer_pending = [true; 2];

        int2.write(DeviceAddr::new(TIMER_ACKNOWLEDGE), &[1])
            .unwrap();
        assert_eq!(int2.timer_pending, [false, true]);
        int2.write(DeviceAddr::new(TIMER_ACKNOWLEDGE), &[2])
            .unwrap();
        assert_eq!(int2.timer_pending, [false, false]);
        assert_eq!(
            int2.read(DeviceAddr::new(TIMER_ACKNOWLEDGE), &mut [0]),
            Err(BusFault::UnsupportedAccess)
        );
    }

    #[test]
    fn programmable_clock_accepts_the_prom_sequence() {
        let mut int2 = Int2::new();

        for value in [0x02, 0x42, 0x42] {
            int2.write(DeviceAddr::new(PROGRAMMABLE_TIMER_CLOCK), &[value])
                .unwrap();
        }

        assert_eq!(int2.timer_programming, [0x02, 0x42, 0x42]);
        assert_eq!(int2.timer_programming_length, 3);
        assert_eq!(int2.timer_pending, [false; 2]);
    }

    #[test]
    fn reset_clears_mutable_state() {
        let mut int2 = Int2::new();
        int2.write(DeviceAddr::new(LOCAL_INTERRUPT_0_MASK + 3), &[0xa5])
            .unwrap();
        int2.write(DeviceAddr::new(VME_INTERRUPT_1_MASK + 3), &[0x5a])
            .unwrap();
        int2.write(DeviceAddr::new(OUTPUT_PORT + 3), &[0x16])
            .unwrap();
        int2.timer_pending = [true; 2];
        int2.write(DeviceAddr::new(PROGRAMMABLE_TIMER_CLOCK), &[0x42])
            .unwrap();

        int2.reset();

        assert_eq!(read_word(&int2, LOCAL_INTERRUPT_0_MASK), Ok(0));
        assert_eq!(read_word(&int2, VME_INTERRUPT_1_MASK), Ok(0));
        assert_eq!(read_word(&int2, OUTPUT_PORT), Ok(0));
        assert_eq!(int2.timer_pending, [false; 2]);
        assert_eq!(int2.timer_programming_length, 0);
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
            int2.read(DeviceAddr::new(0x24), &mut [0]),
            Err(BusFault::Unmapped)
        );
        assert_eq!(read_word(&int2, LOCAL_INTERRUPT_0_MASK), Ok(0x5a));
    }
}
