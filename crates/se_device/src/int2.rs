//! Silicon Graphics INT2 interrupt and timer register front end.

mod timer;

use se_core::bus::{BusFault, DeviceAddr};
use se_core::time::VirtualDuration;

use self::timer::{CounterId, ProgrammableTimer};

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
const SYSTEM_TIMER_COUNTER_0: u64 = 0x33;
const SYSTEM_TIMER_COUNTER_1: u64 = 0x37;
const SYSTEM_TIMER_COUNTER_2: u64 = 0x3b;
const SYSTEM_TIMER_CONTROL: u64 = 0x3f;
const REGISTER_BYTES: u64 = 4;
const OUTPUT_BITS: u8 = 0x1f;

/// The software-visible INT2 state used by the IP12 machine.
pub struct Int2 {
    local_interrupt_status: [u8; 2],
    local_interrupt_masks: [u8; 2],
    vme_interrupt_masks: [u8; 2],
    output_port: u8,
    timer_pending: [bool; 2],
    timer: ProgrammableTimer,
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
            local_interrupt_status: [0; 2],
            local_interrupt_masks: [0; 2],
            vme_interrupt_masks: [0; 2],
            output_port: 0,
            timer_pending: [false; 2],
            timer: ProgrammableTimer::new(),
        }
    }

    /// Restores the mutable INT2 reset state.
    pub fn reset(&mut self) {
        self.local_interrupt_status = [0; 2];
        self.local_interrupt_masks = [0; 2];
        self.vme_interrupt_masks = [0; 2];
        self.output_port = 0;
        self.timer_pending = [false; 2];
        self.timer.reset();
    }

    /// Drives selected local-interrupt-zero input lines.
    pub fn set_local_interrupt_0_input(&mut self, lines: u8, asserted: bool) {
        if asserted {
            self.local_interrupt_status[0] |= lines;
        } else {
            self.local_interrupt_status[0] &= !lines;
        }
    }

    /// Drives selected local-interrupt-one input lines.
    pub fn set_local_interrupt_1_input(&mut self, lines: u8, asserted: bool) {
        if asserted {
            self.local_interrupt_status[1] |= lines;
        } else {
            self.local_interrupt_status[1] &= !lines;
        }
    }

    /// Reports the masked local-interrupt-zero output level.
    #[must_use]
    pub const fn local_interrupt_0_asserted(&self) -> bool {
        self.local_interrupt_status[0] & self.local_interrupt_masks[0] != 0
    }

    /// Reports the masked local-interrupt-one output level.
    #[must_use]
    pub const fn local_interrupt_1_asserted(&self) -> bool {
        self.local_interrupt_status[1] & self.local_interrupt_masks[1] != 0
    }

    /// Reports whether the timer-zero interrupt output is asserted.
    #[must_use]
    pub const fn timer_0_interrupt_asserted(&self) -> bool {
        self.timer_pending[0]
    }

    /// Reports whether the timer-one interrupt output is asserted.
    #[must_use]
    pub const fn timer_1_interrupt_asserted(&self) -> bool {
        self.timer_pending[1]
    }

    /// Advances the one-megahertz system timer by guest virtual time.
    pub fn advance_time(&mut self, elapsed: VirtualDuration) {
        let outputs = self.timer.advance_time(elapsed);
        self.timer_pending[0] |= outputs.counter_0;
        self.timer_pending[1] |= outputs.counter_1;
    }

    /// Returns the virtual duration until the next timer output that can set a
    /// pending interrupt latch.
    #[must_use]
    pub fn time_until_event(&self) -> Option<VirtualDuration> {
        let timer_0 = if self.timer_pending[0] {
            None
        } else {
            self.timer.time_until_output(CounterId::Zero)
        };
        let timer_1 = if self.timer_pending[1] {
            None
        } else {
            self.timer.time_until_output(CounterId::One)
        };
        match (timer_0, timer_1) {
            (Some(timer_0), Some(timer_1)) => Some(timer_0.min(timer_1)),
            (Some(timer), None) | (None, Some(timer)) => Some(timer),
            (None, None) => None,
        }
    }

    /// Reads one fixed-width device-local transaction.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault`] when the complete transaction is not mapped or the
    /// requested width or direction is unsupported.
    pub fn read(&mut self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusFault> {
        let (start, end) = transaction_bounds(address, data.len())?;

        if let Some((register, offset)) = decode_word_register(start, end) {
            let value = match register {
                Register::LocalInterrupt0Status => self.local_interrupt_status[0],
                Register::LocalInterrupt1Status => self.local_interrupt_status[1],
                Register::VmeInterruptStatus => 0,
                Register::LocalInterrupt0Mask => self.local_interrupt_masks[0],
                Register::LocalInterrupt1Mask => self.local_interrupt_masks[1],
                Register::VmeInterrupt0Mask => self.vme_interrupt_masks[0],
                Register::VmeInterrupt1Mask => self.vme_interrupt_masks[1],
                Register::OutputPort => self.output_port,
            };
            read_register(u32::from(value), offset, data);
            return Ok(());
        }

        if start == TIMER_ACKNOWLEDGE
            || start == PROGRAMMABLE_TIMER_CLOCK
            || start == SYSTEM_TIMER_CONTROL
        {
            return Err(BusFault::UnsupportedAccess);
        }

        if let Some(counter) = decode_timer_counter(start) {
            if end != start + 1 {
                return Err(BusFault::UnsupportedAccess);
            }
            data[0] = self.timer.read_counter(counter)?;
            return Ok(());
        }

        Err(BusFault::Unmapped)
    }

    /// Reads one fixed-width transaction without changing timer read state.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault`] when the complete transaction is not mapped or the
    /// requested width or direction is unsupported.
    pub fn debug_read(&self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusFault> {
        let (start, end) = transaction_bounds(address, data.len())?;

        if let Some((register, offset)) = decode_word_register(start, end) {
            let value = match register {
                Register::LocalInterrupt0Status => self.local_interrupt_status[0],
                Register::LocalInterrupt1Status => self.local_interrupt_status[1],
                Register::VmeInterruptStatus => 0,
                Register::LocalInterrupt0Mask => self.local_interrupt_masks[0],
                Register::LocalInterrupt1Mask => self.local_interrupt_masks[1],
                Register::VmeInterrupt0Mask => self.vme_interrupt_masks[0],
                Register::VmeInterrupt1Mask => self.vme_interrupt_masks[1],
                Register::OutputPort => self.output_port,
            };
            read_register(u32::from(value), offset, data);
            return Ok(());
        }

        if start == TIMER_ACKNOWLEDGE
            || start == PROGRAMMABLE_TIMER_CLOCK
            || start == SYSTEM_TIMER_CONTROL
        {
            return Err(BusFault::UnsupportedAccess);
        }

        if let Some(counter) = decode_timer_counter(start) {
            if end != start + 1 {
                return Err(BusFault::UnsupportedAccess);
            }
            data[0] = self.timer.debug_read_counter(counter)?;
            return Ok(());
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
            return Ok(());
        }

        if let Some(counter) = decode_timer_counter(start) {
            if end != start + 1 {
                return Err(BusFault::UnsupportedAccess);
            }
            return self.timer.write_counter(counter, data[0]);
        }

        if start == SYSTEM_TIMER_CONTROL && end == start + 1 {
            return self.timer.write_control(data[0]);
        }

        if start == TIMER_ACKNOWLEDGE
            || start == PROGRAMMABLE_TIMER_CLOCK
            || decode_timer_counter(start).is_some()
            || start == SYSTEM_TIMER_CONTROL
        {
            return Err(BusFault::UnsupportedAccess);
        }

        Err(BusFault::Unmapped)
    }
}

const fn decode_timer_counter(address: u64) -> Option<CounterId> {
    match address {
        SYSTEM_TIMER_COUNTER_0 => Some(CounterId::Zero),
        SYSTEM_TIMER_COUNTER_1 => Some(CounterId::One),
        SYSTEM_TIMER_COUNTER_2 => Some(CounterId::Two),
        _ => None,
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
    use se_core::time::VirtualDuration;

    use super::{
        Int2, LOCAL_INTERRUPT_0_MASK, LOCAL_INTERRUPT_0_STATUS, LOCAL_INTERRUPT_1_MASK,
        LOCAL_INTERRUPT_1_STATUS, OUTPUT_PORT, PROGRAMMABLE_TIMER_CLOCK, SYSTEM_TIMER_CONTROL,
        SYSTEM_TIMER_COUNTER_0, SYSTEM_TIMER_COUNTER_1, SYSTEM_TIMER_COUNTER_2, TIMER_ACKNOWLEDGE,
        VME_INTERRUPT_0_MASK, VME_INTERRUPT_1_MASK, VME_INTERRUPT_STATUS,
    };

    const ATTOSECONDS_PER_MICROSECOND: u128 = 1_000_000_000_000;

    fn read_word(int2: &mut Int2, address: u64) -> Result<u32, BusFault> {
        let mut bytes = [0; 4];
        int2.read(DeviceAddr::new(address), &mut bytes)?;
        Ok(u32::from_be_bytes(bytes))
    }

    #[test]
    fn reset_values_are_inactive() {
        let mut int2 = Int2::new();

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
            assert_eq!(read_word(&mut int2, address), Ok(0));
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
            assert_eq!(read_word(&mut int2, address), Ok(u32::from(value)));
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
            assert_eq!(read_word(&mut int2, address), Ok(0));
        }
    }

    #[test]
    fn local_interrupt_zero_combines_live_inputs_with_the_guest_mask() {
        let mut int2 = Int2::new();
        int2.set_local_interrupt_0_input(1 << 5, true);

        assert_eq!(read_word(&mut int2, LOCAL_INTERRUPT_0_STATUS), Ok(1 << 5));
        assert!(!int2.local_interrupt_0_asserted());

        int2.write(DeviceAddr::new(LOCAL_INTERRUPT_0_MASK + 3), &[1 << 5])
            .unwrap();
        assert!(int2.local_interrupt_0_asserted());

        int2.set_local_interrupt_0_input(1 << 5, false);
        assert_eq!(read_word(&mut int2, LOCAL_INTERRUPT_0_STATUS), Ok(0));
        assert!(!int2.local_interrupt_0_asserted());
    }

    #[test]
    fn local_interrupt_one_is_masked_and_independent_from_local_interrupt_zero() {
        let mut int2 = Int2::new();
        int2.set_local_interrupt_0_input(1 << 5, true);
        int2.set_local_interrupt_1_input(1 << 4, true);

        int2.write(DeviceAddr::new(LOCAL_INTERRUPT_1_MASK + 3), &[1 << 4])
            .unwrap();
        assert!(!int2.local_interrupt_0_asserted());
        assert!(int2.local_interrupt_1_asserted());
        assert_eq!(read_word(&mut int2, LOCAL_INTERRUPT_1_STATUS), Ok(1 << 4));

        int2.write(DeviceAddr::new(LOCAL_INTERRUPT_0_MASK + 3), &[1 << 5])
            .unwrap();
        assert!(int2.local_interrupt_0_asserted());
        assert!(int2.local_interrupt_1_asserted());

        int2.set_local_interrupt_1_input(1 << 4, false);
        assert!(int2.local_interrupt_0_asserted());
        assert!(!int2.local_interrupt_1_asserted());
    }

    #[test]
    fn output_port_keeps_only_the_low_five_bits() {
        let mut int2 = Int2::new();

        int2.write(DeviceAddr::new(OUTPUT_PORT + 3), &[0xf6])
            .unwrap();

        assert_eq!(read_word(&mut int2, OUTPUT_PORT), Ok(0x16));
    }

    #[test]
    fn timer_acknowledge_clears_selected_pending_flags() {
        let mut int2 = Int2::new();
        int2.timer_pending = [true; 2];

        int2.write(DeviceAddr::new(TIMER_ACKNOWLEDGE), &[0])
            .unwrap();
        assert_eq!(int2.timer_pending, [true, true]);
        int2.write(DeviceAddr::new(TIMER_ACKNOWLEDGE), &[1])
            .unwrap();
        assert_eq!(int2.timer_pending, [false, true]);
        int2.write(DeviceAddr::new(TIMER_ACKNOWLEDGE), &[0xfe])
            .unwrap();
        assert_eq!(int2.timer_pending, [false, false]);
        int2.timer_pending = [true; 2];
        int2.write(DeviceAddr::new(TIMER_ACKNOWLEDGE), &[3])
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
        int2.timer_pending = [true, false];
        let timer_before = int2.timer.clone();

        for value in [0x02, 0x42, 0x42] {
            int2.write(DeviceAddr::new(PROGRAMMABLE_TIMER_CLOCK), &[value])
                .unwrap();
        }

        assert_eq!(int2.timer, timer_before);
        assert_eq!(int2.timer_pending, [true, false]);
        assert_eq!(int2.time_until_event(), None);
    }

    #[test]
    fn system_timer_counts_and_returns_a_stable_latch() {
        let mut int2 = Int2::new();

        int2.write(DeviceAddr::new(SYSTEM_TIMER_CONTROL), &[0xb4])
            .unwrap();
        int2.write(DeviceAddr::new(SYSTEM_TIMER_COUNTER_2), &[0x10])
            .unwrap();
        int2.write(DeviceAddr::new(SYSTEM_TIMER_COUNTER_2), &[0x27])
            .unwrap();
        int2.advance_time(VirtualDuration::from_attoseconds(123_456_789));
        int2.advance_time(VirtualDuration::from_attoseconds(
            2_345 * ATTOSECONDS_PER_MICROSECOND - 123_456_789,
        ));
        int2.write(DeviceAddr::new(SYSTEM_TIMER_CONTROL), &[0x80])
            .unwrap();
        int2.advance_time(VirtualDuration::from_attoseconds(
            10 * ATTOSECONDS_PER_MICROSECOND,
        ));

        let mut low = [0xff];
        let mut high = [0xff];
        assert_eq!(
            int2.read(DeviceAddr::new(SYSTEM_TIMER_COUNTER_2), &mut low),
            Ok(())
        );
        assert_eq!(
            int2.read(DeviceAddr::new(SYSTEM_TIMER_COUNTER_2), &mut high),
            Ok(())
        );
        assert_eq!(u16::from_le_bytes([low[0], high[0]]), 7_655);
        int2.write(DeviceAddr::new(SYSTEM_TIMER_CONTROL), &[0x80])
            .unwrap();
        let mut next = [0; 2];
        int2.read(DeviceAddr::new(SYSTEM_TIMER_COUNTER_2), &mut next[..1])
            .unwrap();
        int2.read(DeviceAddr::new(SYSTEM_TIMER_COUNTER_2), &mut next[1..])
            .unwrap();
        assert_eq!(u16::from_le_bytes(next), 7_645);
    }

    #[test]
    fn debug_read_does_not_consume_the_latched_byte_order() {
        let mut int2 = Int2::new();
        int2.write(DeviceAddr::new(SYSTEM_TIMER_CONTROL), &[0xb4])
            .unwrap();
        int2.write(DeviceAddr::new(SYSTEM_TIMER_COUNTER_2), &[0x34])
            .unwrap();
        int2.write(DeviceAddr::new(SYSTEM_TIMER_COUNTER_2), &[0x12])
            .unwrap();
        int2.write(DeviceAddr::new(SYSTEM_TIMER_CONTROL), &[0x80])
            .unwrap();
        let mut debug = [0];

        int2.debug_read(DeviceAddr::new(SYSTEM_TIMER_COUNTER_2), &mut debug)
            .unwrap();
        assert_eq!(debug, [0x34]);

        let mut low = [0];
        int2.read(DeviceAddr::new(SYSTEM_TIMER_COUNTER_2), &mut low)
            .unwrap();
        assert_eq!(low, [0x34]);
    }

    #[test]
    fn all_counter_ports_cascade_to_independent_pending_outputs() {
        let mut int2 = Int2::new();
        for (control, address, reload) in [
            (0xb4, SYSTEM_TIMER_COUNTER_2, 3_u16),
            (0x34, SYSTEM_TIMER_COUNTER_0, 2),
            (0x74, SYSTEM_TIMER_COUNTER_1, 2),
        ] {
            int2.write(DeviceAddr::new(SYSTEM_TIMER_CONTROL), &[control])
                .unwrap();
            for value in reload.to_le_bytes() {
                int2.write(DeviceAddr::new(address), &[value]).unwrap();
            }
        }

        assert_eq!(
            int2.time_until_event(),
            Some(VirtualDuration::from_attoseconds(
                9 * ATTOSECONDS_PER_MICROSECOND
            ))
        );
        int2.advance_time(VirtualDuration::from_attoseconds(
            9 * ATTOSECONDS_PER_MICROSECOND,
        ));

        assert!(int2.timer_0_interrupt_asserted());
        assert!(int2.timer_1_interrupt_asserted());
        assert_eq!(int2.time_until_event(), None);
    }

    #[test]
    fn pending_timers_are_excluded_until_acknowledged() {
        let mut int2 = Int2::new();
        for (control, address, reload) in [
            (0xb4, SYSTEM_TIMER_COUNTER_2, 3_u16),
            (0x34, SYSTEM_TIMER_COUNTER_0, 2),
            (0x74, SYSTEM_TIMER_COUNTER_1, 3),
        ] {
            int2.write(DeviceAddr::new(SYSTEM_TIMER_CONTROL), &[control])
                .unwrap();
            for value in reload.to_le_bytes() {
                int2.write(DeviceAddr::new(address), &[value]).unwrap();
            }
        }

        int2.advance_time(VirtualDuration::from_attoseconds(
            9 * ATTOSECONDS_PER_MICROSECOND,
        ));
        assert_eq!(int2.timer_pending, [true, false]);
        assert_eq!(
            int2.time_until_event(),
            Some(VirtualDuration::from_attoseconds(
                3 * ATTOSECONDS_PER_MICROSECOND
            ))
        );
        int2.advance_time(VirtualDuration::from_attoseconds(
            3 * ATTOSECONDS_PER_MICROSECOND,
        ));
        assert_eq!(int2.timer_pending, [true, true]);
        assert_eq!(int2.time_until_event(), None);

        int2.write(DeviceAddr::new(TIMER_ACKNOWLEDGE), &[1])
            .unwrap();
        assert_eq!(
            int2.time_until_event(),
            Some(VirtualDuration::from_attoseconds(
                3 * ATTOSECONDS_PER_MICROSECOND
            ))
        );
    }

    #[test]
    fn timer_ports_require_exact_byte_transactions_and_latched_reads() {
        let mut int2 = Int2::new();

        for address in [
            SYSTEM_TIMER_COUNTER_0,
            SYSTEM_TIMER_COUNTER_1,
            SYSTEM_TIMER_COUNTER_2,
        ] {
            assert_eq!(
                int2.read(DeviceAddr::new(address), &mut [0]),
                Err(BusFault::UnsupportedAccess)
            );
            assert_eq!(
                int2.write(DeviceAddr::new(address), &[0, 0]),
                Err(BusFault::UnsupportedAccess)
            );
        }
        assert_eq!(
            int2.read(DeviceAddr::new(SYSTEM_TIMER_CONTROL), &mut [0]),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(
            int2.write(DeviceAddr::new(SYSTEM_TIMER_CONTROL), &[0, 0]),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(
            int2.read(DeviceAddr::new(PROGRAMMABLE_TIMER_CLOCK), &mut [0]),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(
            int2.read(DeviceAddr::new(SYSTEM_TIMER_COUNTER_0 + 1), &mut [0]),
            Err(BusFault::Unmapped)
        );
    }

    #[test]
    fn running_counter_rejects_data_only_reload_without_losing_state() {
        let mut int2 = Int2::new();
        int2.write(DeviceAddr::new(SYSTEM_TIMER_CONTROL), &[0xb4])
            .unwrap();
        int2.write(DeviceAddr::new(SYSTEM_TIMER_COUNTER_2), &[3])
            .unwrap();
        int2.write(DeviceAddr::new(SYSTEM_TIMER_COUNTER_2), &[0])
            .unwrap();
        let timer_before = int2.timer.clone();

        assert_eq!(
            int2.write(DeviceAddr::new(SYSTEM_TIMER_COUNTER_2), &[4]),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(int2.timer, timer_before);
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
        int2.write(DeviceAddr::new(SYSTEM_TIMER_CONTROL), &[0xb4])
            .unwrap();
        int2.write(DeviceAddr::new(SYSTEM_TIMER_COUNTER_2), &[0x27])
            .unwrap();
        int2.advance_time(VirtualDuration::from_attoseconds(123));
        int2.set_local_interrupt_0_input(1 << 5, true);
        int2.set_local_interrupt_1_input(1 << 4, true);

        int2.reset();

        assert_eq!(read_word(&mut int2, LOCAL_INTERRUPT_0_MASK), Ok(0));
        assert_eq!(read_word(&mut int2, LOCAL_INTERRUPT_0_STATUS), Ok(0));
        assert_eq!(read_word(&mut int2, LOCAL_INTERRUPT_1_STATUS), Ok(0));
        assert_eq!(read_word(&mut int2, VME_INTERRUPT_1_MASK), Ok(0));
        assert_eq!(read_word(&mut int2, OUTPUT_PORT), Ok(0));
        assert_eq!(int2.timer_pending, [false; 2]);
        assert_eq!(int2.timer, super::timer::ProgrammableTimer::new());
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
        assert_eq!(read_word(&mut int2, LOCAL_INTERRUPT_0_MASK), Ok(0x5a));
    }
}
