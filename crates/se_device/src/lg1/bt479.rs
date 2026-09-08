//! Brooktree Bt479 color palette and DAC control interface.
//!
//! The board reaches this device through the REX configuration bus, which
//! presents an RS2:RS0 selector and one byte of data per access. The selector
//! values match the numbers used by the SGI headers and the PROM.

use serde::{Deserialize, Serialize};

/// Palette entries addressable by the display path.
const PALETTE_ENTRIES: usize = 1024;

/// Palette entries reachable through one host bank.
const BANK_ENTRIES: usize = 256;

/// Overlay palette entries retained for host access.
const OVERLAY_ENTRIES: usize = 16;

/// Bytes in the indirect control address space.
const CONTROL_BYTES: usize = 256;

/// Indirect control address of command register zero.
const COMMAND_REGISTER_0: u8 = 0x82;

/// One selector value presented by the REX configuration bus.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Selector {
    /// Palette and control write address.
    WriteAddress,
    /// Palette color data.
    PaletteData,
    /// Pixel read mask.
    PixelReadMask,
    /// Palette and control read address.
    ReadAddress,
    /// Overlay write address.
    OverlayWriteAddress,
    /// Overlay color data.
    OverlayData,
    /// Indirect control data.
    ControlData,
    /// Overlay read address.
    OverlayReadAddress,
}

impl Selector {
    /// Decodes the three selector bits driven by the configuration bus.
    pub(super) const fn from_bits(bits: u8) -> Self {
        match bits & 0x07 {
            0 => Self::WriteAddress,
            1 => Self::PaletteData,
            2 => Self::PixelReadMask,
            3 => Self::ReadAddress,
            4 => Self::OverlayWriteAddress,
            5 => Self::OverlayData,
            6 => Self::ControlData,
            _ => Self::OverlayReadAddress,
        }
    }
}

/// The Bt479 color palette, overlay registers, and control state.
#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Bt479 {
    palette: Box<[[u8; 3]]>,
    overlay_palette: Box<[[u8; 3]]>,
    control: Box<[u8]>,
    address: u8,
    overlay_address: u8,
    pixel_read_mask: u8,
    /// Index of the next color component transferred by a palette access.
    component: u8,
    /// Red and green bytes held until a blue write commits the entry.
    staged: [u8; 2],
    /// Color prefetched by the read protocol for the next three reads.
    prefetched: [u8; 3],
    /// Index of the next overlay color component transferred.
    overlay_component: u8,
    /// Overlay red and green bytes held until a blue write commits.
    overlay_staged: [u8; 2],
    /// Overlay color prefetched by the read protocol.
    overlay_prefetched: [u8; 3],
}

impl Bt479 {
    /// Creates a Bt479 with cleared palette and control state.
    ///
    /// The data sheet marks the command and flood registers as uninitialized,
    /// so these zeros are emulator storage rather than documented power-on
    /// values.
    pub(super) fn new() -> Self {
        Self {
            palette: vec![[0; 3]; PALETTE_ENTRIES].into_boxed_slice(),
            overlay_palette: vec![[0; 3]; OVERLAY_ENTRIES].into_boxed_slice(),
            control: vec![0; CONTROL_BYTES].into_boxed_slice(),
            address: 0,
            overlay_address: 0,
            pixel_read_mask: 0,
            component: 0,
            staged: [0; 2],
            prefetched: [0; 3],
            overlay_component: 0,
            overlay_staged: [0; 2],
            overlay_prefetched: [0; 3],
        }
    }

    /// Restores the reset state of every host-visible register.
    pub(super) fn reset(&mut self) {
        self.palette.fill([0; 3]);
        self.overlay_palette.fill([0; 3]);
        self.control.fill(0);
        self.address = 0;
        self.overlay_address = 0;
        self.pixel_read_mask = 0;
        self.component = 0;
        self.staged = [0; 2];
        self.prefetched = [0; 3];
        self.overlay_component = 0;
        self.overlay_staged = [0; 2];
        self.overlay_prefetched = [0; 3];
    }

    /// Returns the displayed color for one ten-bit palette index.
    ///
    /// Index bits 9:8 select the palette bank, which is independent of the
    /// bank the host currently accesses.
    pub(super) fn color(&self, index: u16) -> [u8; 3] {
        self.palette[usize::from(index) % PALETTE_ENTRIES]
    }

    /// Writes one byte to the selected interface port.
    pub(super) fn write(&mut self, selector: Selector, value: u8) {
        match selector {
            Selector::WriteAddress => {
                self.address = value;
                self.component = 0;
            }
            Selector::ReadAddress => {
                self.address = value;
                self.component = 0;
                self.prefetch();
            }
            Selector::PaletteData => self.write_color(value),
            Selector::PixelReadMask => self.pixel_read_mask = value,
            Selector::ControlData => {
                self.control[usize::from(self.address)] = value;
                self.address = self.address.wrapping_add(1);
            }
            Selector::OverlayWriteAddress => {
                self.overlay_address = value;
                self.overlay_component = 0;
            }
            Selector::OverlayReadAddress => {
                self.overlay_address = value;
                self.overlay_component = 0;
                self.prefetch_overlay();
            }
            Selector::OverlayData => self.write_overlay_color(value),
        }
    }

    /// Reads one byte from the selected interface port.
    pub(super) fn read(&mut self, selector: Selector) -> u8 {
        match selector {
            Selector::WriteAddress | Selector::ReadAddress => self.address,
            Selector::PaletteData => self.read_color(),
            Selector::PixelReadMask => self.pixel_read_mask,
            Selector::ControlData => {
                let value = self.control[usize::from(self.address)];
                self.address = self.address.wrapping_add(1);
                value
            }
            Selector::OverlayWriteAddress | Selector::OverlayReadAddress => self.overlay_address,
            Selector::OverlayData => self.read_overlay_color(),
        }
    }

    /// Returns the palette entry index selected by the host bank and address.
    ///
    /// Command register zero bits 7:4 encode the bank. Only encodings zero
    /// through three are documented, so the emulator keeps the low two bits;
    /// the remaining encodings have no local evidence.
    fn host_entry(&self) -> usize {
        let bank = usize::from((self.control[usize::from(COMMAND_REGISTER_0)] >> 4) & 0x03);
        bank * BANK_ENTRIES + usize::from(self.address)
    }

    /// Accepts one color component, committing the entry after blue.
    fn write_color(&mut self, value: u8) {
        match self.component {
            0 | 1 => {
                self.staged[usize::from(self.component)] = value;
                self.component += 1;
            }
            _ => {
                let entry = self.host_entry();
                self.palette[entry] = [self.staged[0], self.staged[1], value];
                self.address = self.address.wrapping_add(1);
                self.component = 0;
            }
        }
    }

    /// Returns one prefetched color component, refilling after blue.
    fn read_color(&mut self) -> u8 {
        let value = self.prefetched[usize::from(self.component)];
        if self.component == 2 {
            self.component = 0;
            self.prefetch();
        } else {
            self.component += 1;
        }
        value
    }

    /// Loads the addressed entry and advances the address counter.
    fn prefetch(&mut self) {
        self.prefetched = self.palette[self.host_entry()];
        self.address = self.address.wrapping_add(1);
    }

    /// Accepts one overlay color component, committing the entry after blue.
    fn write_overlay_color(&mut self, value: u8) {
        match self.overlay_component {
            0 | 1 => {
                self.overlay_staged[usize::from(self.overlay_component)] = value;
                self.overlay_component += 1;
            }
            _ => {
                let entry = usize::from(self.overlay_address) % OVERLAY_ENTRIES;
                self.overlay_palette[entry] =
                    [self.overlay_staged[0], self.overlay_staged[1], value];
                self.overlay_address = self.overlay_address.wrapping_add(1);
                self.overlay_component = 0;
            }
        }
    }

    /// Returns one prefetched overlay component, refilling after blue.
    fn read_overlay_color(&mut self) -> u8 {
        let value = self.overlay_prefetched[usize::from(self.overlay_component)];
        if self.overlay_component == 2 {
            self.overlay_component = 0;
            self.prefetch_overlay();
        } else {
            self.overlay_component += 1;
        }
        value
    }

    /// Loads the addressed overlay entry and advances its address counter.
    fn prefetch_overlay(&mut self) {
        self.overlay_prefetched =
            self.overlay_palette[usize::from(self.overlay_address) % OVERLAY_ENTRIES];
        self.overlay_address = self.overlay_address.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::{Bt479, COMMAND_REGISTER_0, Selector};

    /// Writes one palette entry through the host protocol.
    fn write_entry(dac: &mut Bt479, address: u8, color: [u8; 3]) {
        dac.write(Selector::WriteAddress, address);
        for component in color {
            dac.write(Selector::PaletteData, component);
        }
    }

    /// Reads one palette entry through the host protocol.
    fn read_entry(dac: &mut Bt479, address: u8) -> [u8; 3] {
        dac.write(Selector::ReadAddress, address);
        [
            dac.read(Selector::PaletteData),
            dac.read(Selector::PaletteData),
            dac.read(Selector::PaletteData),
        ]
    }

    /// Selects the host palette bank through command register zero.
    fn select_bank(dac: &mut Bt479, bank: u8) {
        dac.write(Selector::WriteAddress, COMMAND_REGISTER_0);
        dac.write(Selector::ControlData, bank << 4);
    }

    #[test]
    fn selector_bits_match_the_documented_ports() {
        assert_eq!(Selector::from_bits(0), Selector::WriteAddress);
        assert_eq!(Selector::from_bits(1), Selector::PaletteData);
        assert_eq!(Selector::from_bits(2), Selector::PixelReadMask);
        assert_eq!(Selector::from_bits(3), Selector::ReadAddress);
        assert_eq!(Selector::from_bits(4), Selector::OverlayWriteAddress);
        assert_eq!(Selector::from_bits(5), Selector::OverlayData);
        assert_eq!(Selector::from_bits(6), Selector::ControlData);
        assert_eq!(Selector::from_bits(7), Selector::OverlayReadAddress);
    }

    #[test]
    fn blue_commits_the_entry_and_advances_the_address() {
        let mut dac = Bt479::new();

        dac.write(Selector::WriteAddress, 4);
        dac.write(Selector::PaletteData, 0x11);
        dac.write(Selector::PaletteData, 0x22);
        assert_eq!(dac.color(4), [0, 0, 0]);
        dac.write(Selector::PaletteData, 0x33);

        assert_eq!(dac.color(4), [0x11, 0x22, 0x33]);
        assert_eq!(dac.read(Selector::WriteAddress), 5);
    }

    #[test]
    fn consecutive_writes_fill_successive_entries() {
        let mut dac = Bt479::new();

        dac.write(Selector::WriteAddress, 0);
        for value in 0..6_u8 {
            dac.write(Selector::PaletteData, value);
        }

        assert_eq!(dac.color(0), [0, 1, 2]);
        assert_eq!(dac.color(1), [3, 4, 5]);
    }

    #[test]
    fn the_read_protocol_prefetches_before_the_first_component() {
        let mut dac = Bt479::new();
        write_entry(&mut dac, 9, [1, 2, 3]);
        write_entry(&mut dac, 10, [4, 5, 6]);

        dac.write(Selector::ReadAddress, 9);
        assert_eq!(dac.read(Selector::ReadAddress), 10);
        assert_eq!(
            [
                dac.read(Selector::PaletteData),
                dac.read(Selector::PaletteData),
                dac.read(Selector::PaletteData),
            ],
            [1, 2, 3]
        );
        assert_eq!(
            [
                dac.read(Selector::PaletteData),
                dac.read(Selector::PaletteData),
                dac.read(Selector::PaletteData),
            ],
            [4, 5, 6]
        );
    }

    #[test]
    fn host_banks_address_separate_quarters_of_the_palette() {
        let mut dac = Bt479::new();

        for bank in 0..4_u8 {
            select_bank(&mut dac, bank);
            write_entry(&mut dac, 7, [bank, bank + 16, bank + 32]);
        }

        for bank in 0..4_u16 {
            let expected = [bank as u8, bank as u8 + 16, bank as u8 + 32];
            assert_eq!(dac.color(bank * 256 + 7), expected);
            select_bank(&mut dac, bank as u8);
            assert_eq!(read_entry(&mut dac, 7), expected);
        }
    }

    #[test]
    fn the_address_wraps_inside_one_bank() {
        let mut dac = Bt479::new();
        select_bank(&mut dac, 1);

        write_entry(&mut dac, 0xff, [1, 2, 3]);
        dac.write(Selector::PaletteData, 4);
        dac.write(Selector::PaletteData, 5);
        dac.write(Selector::PaletteData, 6);

        assert_eq!(dac.color(0x1ff), [1, 2, 3]);
        assert_eq!(dac.color(0x100), [4, 5, 6]);
        assert_eq!(dac.color(0x000), [0, 0, 0]);
    }

    #[test]
    fn writing_an_address_resets_the_component_phase() {
        let mut dac = Bt479::new();

        dac.write(Selector::WriteAddress, 2);
        dac.write(Selector::PaletteData, 0x77);
        dac.write(Selector::WriteAddress, 2);
        dac.write(Selector::PaletteData, 1);
        dac.write(Selector::PaletteData, 2);
        dac.write(Selector::PaletteData, 3);

        assert_eq!(dac.color(2), [1, 2, 3]);
    }

    #[test]
    fn reading_the_address_register_preserves_the_component_phase() {
        let mut dac = Bt479::new();
        write_entry(&mut dac, 1, [9, 8, 7]);

        dac.write(Selector::ReadAddress, 1);
        assert_eq!(dac.read(Selector::PaletteData), 9);
        assert_eq!(dac.read(Selector::ReadAddress), 2);
        assert_eq!(dac.read(Selector::PaletteData), 8);
        assert_eq!(dac.read(Selector::PaletteData), 7);
    }

    #[test]
    fn control_access_advances_one_byte_at_a_time() {
        let mut dac = Bt479::new();

        dac.write(Selector::WriteAddress, COMMAND_REGISTER_0);
        dac.write(Selector::ControlData, 0x0f);
        dac.write(Selector::ControlData, 0x02);

        dac.write(Selector::WriteAddress, COMMAND_REGISTER_0);
        assert_eq!(dac.read(Selector::ControlData), 0x0f);
        assert_eq!(dac.read(Selector::ControlData), 0x02);
    }

    #[test]
    fn the_pixel_read_mask_round_trips() {
        let mut dac = Bt479::new();

        dac.write(Selector::PixelReadMask, 0xff);

        assert_eq!(dac.read(Selector::PixelReadMask), 0xff);
    }

    #[test]
    fn overlay_registers_round_trip_without_touching_the_palette() {
        let mut dac = Bt479::new();
        write_entry(&mut dac, 0, [1, 2, 3]);

        dac.write(Selector::OverlayWriteAddress, 1);
        for value in [0x40, 0x50, 0x60] {
            dac.write(Selector::OverlayData, value);
        }
        dac.write(Selector::OverlayReadAddress, 1);

        assert_eq!(
            [
                dac.read(Selector::OverlayData),
                dac.read(Selector::OverlayData),
                dac.read(Selector::OverlayData),
            ],
            [0x40, 0x50, 0x60]
        );
        assert_eq!(dac.color(0), [1, 2, 3]);
    }

    #[test]
    fn reset_clears_the_palette_and_protocol_state() {
        let mut dac = Bt479::new();
        select_bank(&mut dac, 2);
        write_entry(&mut dac, 5, [1, 2, 3]);
        dac.write(Selector::PixelReadMask, 0xff);

        dac.reset();

        assert_eq!(dac.color(0x205), [0, 0, 0]);
        assert_eq!(dac.read(Selector::PixelReadMask), 0);
        assert_eq!(dac.read(Selector::WriteAddress), 0);
    }
}
