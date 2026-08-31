//! National Semiconductor NMC93CS46 serial EEPROM.

const WORD_COUNT: usize = 64;
const COMMAND_BITS: u8 = 11;
const DATA_BITS: u8 = 16;

const READ_COMMAND: u16 = 0x0600;
const WRITE_COMMAND: u16 = 0x0500;
const WRITE_ENABLE_COMMAND: u16 = 0x04c0;
const WRITE_DISABLE_COMMAND: u16 = 0x0400;
const OPERATION_MASK: u16 = 0x07c0;
const ADDRESS_MASK: u16 = 0b11_1111;

#[derive(Clone, Copy)]
enum Transfer {
    Command {
        bits: u16,
        count: u8,
    },
    Read {
        value: u16,
        next_bit: u8,
    },
    Write {
        address: usize,
        value: u16,
        count: u8,
    },
    Ignore,
}

/// A 64-word serial EEPROM with the command format used by the IP12.
pub struct Nmc93cs46 {
    words: [u16; WORD_COUNT],
    transfer: Transfer,
    write_enabled: bool,
    chip_select: bool,
    clock: bool,
    data_out: bool,
}

impl Nmc93cs46 {
    /// Creates an erased EEPROM.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            words: [u16::MAX; WORD_COUNT],
            transfer: Transfer::Command { bits: 0, count: 0 },
            write_enabled: false,
            chip_select: false,
            clock: false,
            data_out: false,
        }
    }

    /// Resets the serial interface without erasing stored words.
    pub fn reset(&mut self) {
        self.transfer = Transfer::Command { bits: 0, count: 0 };
        self.write_enabled = false;
        self.chip_select = false;
        self.clock = false;
        self.data_out = false;
    }

    /// Applies the current values of the four serial input pins.
    pub fn drive_pins(&mut self, _pre: bool, chip_select: bool, clock: bool, data_in: bool) {
        if !chip_select {
            self.transfer = Transfer::Command { bits: 0, count: 0 };
            self.chip_select = false;
            self.clock = clock;
            self.data_out = false;
            return;
        }

        if !self.chip_select {
            self.transfer = Transfer::Command { bits: 0, count: 0 };
            self.data_out = true;
        }

        let rising_edge = !self.clock && clock;
        self.chip_select = true;
        self.clock = clock;

        if rising_edge {
            self.clock_rising_edge(data_in);
        }
    }

    /// Returns the current serial data output pin.
    #[must_use]
    pub const fn data_out(&self) -> bool {
        self.chip_select && self.data_out
    }

    fn clock_rising_edge(&mut self, data_in: bool) {
        match self.transfer {
            Transfer::Command { bits, count } => {
                if count == 0 && !data_in {
                    self.data_out = true;
                    return;
                }

                let bits = bits << 1 | u16::from(data_in);
                let count = count + 1;
                if count == COMMAND_BITS {
                    self.decode_command(bits);
                } else {
                    self.transfer = Transfer::Command { bits, count };
                    self.data_out = true;
                }
            }
            Transfer::Read { value, next_bit } => {
                self.data_out = value & (1 << (DATA_BITS - 1 - next_bit)) != 0;
                self.transfer = if next_bit + 1 == DATA_BITS {
                    Transfer::Ignore
                } else {
                    Transfer::Read {
                        value,
                        next_bit: next_bit + 1,
                    }
                };
            }
            Transfer::Write {
                address,
                value,
                count,
            } => {
                let value = value << 1 | u16::from(data_in);
                let count = count + 1;
                if count == DATA_BITS {
                    if self.write_enabled {
                        self.words[address] = value;
                    }
                    self.transfer = Transfer::Ignore;
                    self.data_out = true;
                } else {
                    self.transfer = Transfer::Write {
                        address,
                        value,
                        count,
                    };
                }
            }
            Transfer::Ignore => {}
        }
    }

    fn decode_command(&mut self, command: u16) {
        let address = usize::from(command & ADDRESS_MASK);
        match command & OPERATION_MASK {
            READ_COMMAND => {
                self.transfer = Transfer::Read {
                    value: self.words[address],
                    next_bit: 0,
                };
            }
            WRITE_COMMAND => {
                self.transfer = Transfer::Write {
                    address,
                    value: 0,
                    count: 0,
                };
            }
            WRITE_ENABLE_COMMAND if command == WRITE_ENABLE_COMMAND => {
                self.write_enabled = true;
                self.transfer = Transfer::Ignore;
            }
            WRITE_DISABLE_COMMAND if command == WRITE_DISABLE_COMMAND => {
                self.write_enabled = false;
                self.transfer = Transfer::Ignore;
            }
            _ => {
                self.transfer = Transfer::Ignore;
            }
        }
        self.data_out = true;
    }
}

impl Default for Nmc93cs46 {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::Nmc93cs46;

    const READ: u16 = 0x0600;
    const WRITE: u16 = 0x0500;
    const WRITE_ENABLE: u16 = 0x04c0;
    const WRITE_DISABLE: u16 = 0x0400;

    fn select(device: &mut Nmc93cs46) {
        device.drive_pins(false, true, false, false);
    }

    fn deselect(device: &mut Nmc93cs46) {
        device.drive_pins(false, false, false, false);
    }

    fn clock_bit(device: &mut Nmc93cs46, bit: bool) -> bool {
        device.drive_pins(false, true, false, bit);
        device.drive_pins(false, true, true, bit);
        device.data_out()
    }

    fn shift_command(device: &mut Nmc93cs46, command: u16) {
        for bit in (0..11).rev() {
            clock_bit(device, command & (1 << bit) != 0);
        }
    }

    fn read_word(device: &mut Nmc93cs46, address: u16) -> u16 {
        select(device);
        shift_command(device, READ | address);
        let mut value = 0;
        for _ in 0..16 {
            value = value << 1 | u16::from(clock_bit(device, false));
        }
        deselect(device);
        value
    }

    fn write_command(device: &mut Nmc93cs46, command: u16) {
        select(device);
        shift_command(device, command);
        deselect(device);
    }

    fn write_word(device: &mut Nmc93cs46, address: u16, value: u16) {
        select(device);
        shift_command(device, WRITE | address);
        for bit in (0..16).rev() {
            clock_bit(device, value & (1 << bit) != 0);
        }
        deselect(device);
    }

    #[test]
    fn erased_words_read_as_all_ones() {
        let mut device = Nmc93cs46::new();

        for address in [0, 1, 31, 63] {
            assert_eq!(read_word(&mut device, address), u16::MAX);
        }
    }

    #[test]
    fn command_collection_ignores_leading_zero_clocks() {
        let mut device = Nmc93cs46::new();
        select(&mut device);

        for _ in 0..5 {
            assert!(clock_bit(&mut device, false));
        }
        shift_command(&mut device, READ | 7);

        let mut value = 0;
        for _ in 0..16 {
            value = value << 1 | u16::from(clock_bit(&mut device, false));
        }
        assert_eq!(value, u16::MAX);
    }

    #[test]
    fn nine_command_bits_leave_the_device_ready_for_two_more_bits() {
        let mut device = Nmc93cs46::new();
        select(&mut device);

        for bit in (2..11).rev() {
            clock_bit(&mut device, READ & (1 << bit) != 0);
        }
        assert!(device.data_out());
        clock_bit(&mut device, false);
        clock_bit(&mut device, false);
        assert!(clock_bit(&mut device, false));
    }

    #[test]
    fn write_enable_write_read_and_disable_follow_msb_first_order() {
        let mut device = Nmc93cs46::new();

        write_command(&mut device, WRITE_ENABLE);
        write_word(&mut device, 0x15, 0x8123);
        assert_eq!(read_word(&mut device, 0x15), 0x8123);

        write_command(&mut device, WRITE_DISABLE);
        write_word(&mut device, 0x15, 0x4567);
        assert_eq!(read_word(&mut device, 0x15), 0x8123);
    }

    #[test]
    fn writes_are_ignored_until_enabled() {
        let mut device = Nmc93cs46::new();

        write_word(&mut device, 3, 0);

        assert_eq!(read_word(&mut device, 3), u16::MAX);
    }

    #[test]
    fn deselect_aborts_incomplete_commands_and_writes() {
        let mut device = Nmc93cs46::new();
        write_command(&mut device, WRITE_ENABLE);

        select(&mut device);
        for bit in (5..11).rev() {
            clock_bit(&mut device, (WRITE | 9) & (1 << bit) != 0);
        }
        deselect(&mut device);

        select(&mut device);
        shift_command(&mut device, WRITE | 9);
        for bit in (8..16).rev() {
            clock_bit(&mut device, 0x1234 & (1 << bit) != 0);
        }
        deselect(&mut device);

        assert_eq!(read_word(&mut device, 9), u16::MAX);
    }

    #[test]
    fn reset_preserves_contents_and_disables_writes() {
        let mut device = Nmc93cs46::new();
        write_command(&mut device, WRITE_ENABLE);
        write_word(&mut device, 63, 0x5aa5);

        device.reset();

        assert!(!device.data_out());
        assert_eq!(read_word(&mut device, 63), 0x5aa5);
        write_word(&mut device, 63, 0);
        assert_eq!(read_word(&mut device, 63), 0x5aa5);
    }
}
