//! Display composition from board state to finished video pixels.
//!
//! Composition is a board-level task rather than a chip-level one: it reads
//! the frame buffer planes, the VC1 display mode and cursor state, and the
//! Bt479 palette together. The result is the RGBA8888 image the host shows.

use super::bt479::Bt479;
use super::vc1::{DISPLAY_HEIGHT, DISPLAY_WIDTH, Vc1};
use super::vram::Vram;

/// Bytes occupied by one composed frame.
pub(super) const FRAME_BYTES: usize = (DISPLAY_WIDTH * DISPLAY_HEIGHT) as usize * 4;

/// Width and height of the hardware cursor in pixels.
const CURSOR_EXTENT: u32 = 32;

/// Bits of cursor bitmap data per pixel.
const CURSOR_BITS_PER_PIXEL: u32 = 2;

/// Palette map selecting the cursor color entries.
const CURSOR_MAP: u16 = 3;

/// Composes one complete frame into `pixels`.
///
/// The buffer must hold [`FRAME_BYTES`] bytes. Every pixel is written, so no
/// data from an earlier frame survives.
///
/// # Panics
///
/// Panics when `pixels` is not exactly [`FRAME_BYTES`] long.
pub(super) fn compose(vram: &Vram, vc1: &Vc1, dac: &Bt479, pixels: &mut [u8]) {
    assert_eq!(
        pixels.len(),
        FRAME_BYTES,
        "composition target must hold one whole frame"
    );

    // Every scan line currently uses display identifier zero. The identifier
    // tables are stored but not interpreted, so the whole screen shares one
    // display mode.
    let index_base = color_index_base(vc1);

    for y in 0..DISPLAY_HEIGHT {
        let source = vram.pixel_row(y);
        let row = (y * DISPLAY_WIDTH) as usize * 4;
        for x in 0..DISPLAY_WIDTH {
            let index = index_base | u16::from(source[x as usize]);
            let [red, green, blue] = dac.color(index);
            let offset = row + x as usize * 4;
            pixels[offset] = red;
            pixels[offset + 1] = green;
            pixels[offset + 2] = blue;
            pixels[offset + 3] = u8::MAX;
        }
    }

    compose_cursor(vc1, dac, pixels);
}

/// Fills the frame with black while video timing stays valid.
///
/// # Panics
///
/// Panics when `pixels` is not exactly [`FRAME_BYTES`] long.
pub(super) fn compose_blank(pixels: &mut [u8]) {
    assert_eq!(
        pixels.len(),
        FRAME_BYTES,
        "composition target must hold one whole frame"
    );

    for pixel in pixels.chunks_exact_mut(4) {
        pixel[0] = 0;
        pixel[1] = 0;
        pixel[2] = 0;
        pixel[3] = u8::MAX;
    }
}

/// Returns the palette map bits contributed by the display mode.
///
/// The technical report describes XMAP as combining the display identifier
/// and the pixel stream into a ten-bit palette index. Only eight-bit color
/// index mode is interpreted, so the map bits stay zero and the pixel byte
/// supplies the rest of the index.
const fn color_index_base(_vc1: &Vc1) -> u16 {
    0
}

/// Draws the hardware cursor over the composed image.
fn compose_cursor(vc1: &Vc1, dac: &Bt479, pixels: &mut [u8]) {
    let Some(cursor) = vc1.cursor() else {
        return;
    };

    let foreground = dac.color((CURSOR_MAP << 8) | 1);
    let background = dac.color((CURSOR_MAP << 8) | 2);

    for row in 0..CURSOR_EXTENT {
        let y = cursor.top + row as i32;
        if y < 0 || y >= DISPLAY_HEIGHT as i32 {
            continue;
        }
        for column in 0..CURSOR_EXTENT {
            let x = cursor.left + column as i32;
            if x < 0 || x >= DISPLAY_WIDTH as i32 {
                continue;
            }

            // Cursor pixels are two bits each, packed most significant first.
            let bit = (row * CURSOR_EXTENT + column) * CURSOR_BITS_PER_PIXEL;
            let byte = cursor.bitmap[(bit / 8) as usize];
            let shift = 8 - CURSOR_BITS_PER_PIXEL - (bit % 8);
            let color = match (byte >> shift) & 0x03 {
                1 => foreground,
                2 => background,
                _ => continue,
            };

            let offset = ((y as u32 * DISPLAY_WIDTH) + x as u32) as usize * 4;
            pixels[offset] = color[0];
            pixels[offset + 1] = color[1];
            pixels[offset + 2] = color[2];
            pixels[offset + 3] = u8::MAX;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::bt479::{Bt479, Selector as DacSelector};
    use super::super::vc1::{
        DISPLAY_HEIGHT, DISPLAY_WIDTH, SYS_CTRL_CURSOR_DISPLAY, Selector as Vc1Selector, Vc1,
    };
    use super::super::vram::{PlaneGroup, Vram};
    use super::{FRAME_BYTES, compose, compose_blank};

    /// Writes one palette entry in the host bank currently selected.
    fn write_palette(dac: &mut Bt479, address: u8, color: [u8; 3]) {
        dac.write(DacSelector::WriteAddress, address);
        for component in color {
            dac.write(DacSelector::PaletteData, component);
        }
    }

    /// Selects one host palette bank through command register zero.
    fn select_bank(dac: &mut Bt479, bank: u8) {
        dac.write(DacSelector::WriteAddress, 0x82);
        dac.write(DacSelector::ControlData, bank << 4);
    }

    /// Returns the composed color at one screen coordinate.
    fn pixel_at(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
        let offset = ((y * DISPLAY_WIDTH + x) as usize) * 4;
        [
            pixels[offset],
            pixels[offset + 1],
            pixels[offset + 2],
            pixels[offset + 3],
        ]
    }

    #[test]
    fn pixels_are_looked_up_through_the_first_palette_bank() {
        let mut vram = Vram::new();
        let vc1 = Vc1::new();
        let mut dac = Bt479::new();
        write_palette(&mut dac, 0x21, [0x10, 0x20, 0x30]);
        vram.write_masked(PlaneGroup::Pixel, 5, 7, 0x21, 0xff);
        let mut pixels = vec![0; FRAME_BYTES];

        compose(&vram, &vc1, &dac, &mut pixels);

        assert_eq!(pixel_at(&pixels, 5, 7), [0x10, 0x20, 0x30, 0xff]);
    }

    #[test]
    fn every_pixel_is_opaque_and_the_whole_frame_is_written() {
        let vram = Vram::new();
        let vc1 = Vc1::new();
        let mut dac = Bt479::new();
        write_palette(&mut dac, 0, [1, 2, 3]);
        let mut pixels = vec![0xa5; FRAME_BYTES];

        compose(&vram, &vc1, &dac, &mut pixels);

        assert!(pixels.chunks_exact(4).all(|pixel| pixel == [1, 2, 3, 0xff]));
    }

    #[test]
    fn only_the_visible_rows_reach_the_composed_frame() {
        let mut vram = Vram::new();
        let vc1 = Vc1::new();
        let mut dac = Bt479::new();
        write_palette(&mut dac, 0, [0, 0, 0]);
        write_palette(&mut dac, 9, [9, 9, 9]);
        // An off-screen row must not appear anywhere in the frame.
        vram.write_masked(PlaneGroup::Pixel, 0, DISPLAY_HEIGHT, 9, 0xff);
        let mut pixels = vec![0; FRAME_BYTES];

        compose(&vram, &vc1, &dac, &mut pixels);

        assert!(pixels.chunks_exact(4).all(|pixel| pixel == [0, 0, 0, 0xff]));
    }

    #[test]
    fn a_palette_change_reinterprets_stored_pixels() {
        let mut vram = Vram::new();
        let vc1 = Vc1::new();
        let mut dac = Bt479::new();
        vram.write_masked(PlaneGroup::Pixel, 1, 1, 4, 0xff);
        let mut pixels = vec![0; FRAME_BYTES];

        write_palette(&mut dac, 4, [0x11, 0x11, 0x11]);
        compose(&vram, &vc1, &dac, &mut pixels);
        assert_eq!(pixel_at(&pixels, 1, 1), [0x11, 0x11, 0x11, 0xff]);

        write_palette(&mut dac, 4, [0x77, 0x88, 0x99]);
        compose(&vram, &vc1, &dac, &mut pixels);
        assert_eq!(pixel_at(&pixels, 1, 1), [0x77, 0x88, 0x99, 0xff]);
    }

    #[test]
    fn the_host_palette_bank_does_not_move_the_displayed_bank() {
        let mut vram = Vram::new();
        let vc1 = Vc1::new();
        let mut dac = Bt479::new();
        write_palette(&mut dac, 3, [1, 1, 1]);
        select_bank(&mut dac, 2);
        write_palette(&mut dac, 3, [2, 2, 2]);
        vram.write_masked(PlaneGroup::Pixel, 0, 0, 3, 0xff);
        let mut pixels = vec![0; FRAME_BYTES];

        compose(&vram, &vc1, &dac, &mut pixels);

        assert_eq!(pixel_at(&pixels, 0, 0), [1, 1, 1, 0xff]);
    }

    #[test]
    fn a_blank_frame_is_black_and_opaque() {
        let mut pixels = vec![0xa5; FRAME_BYTES];

        compose_blank(&mut pixels);

        assert!(pixels.chunks_exact(4).all(|pixel| pixel == [0, 0, 0, 0xff]));
    }

    #[test]
    fn the_cursor_is_absent_until_the_guest_enables_its_display() {
        let vram = Vram::new();
        let mut vc1 = Vc1::new();
        let mut dac = Bt479::new();
        select_bank(&mut dac, 3);
        write_palette(&mut dac, 1, [0xff, 0, 0]);
        upload_cursor(&mut vc1, 0xff);
        let mut pixels = vec![0; FRAME_BYTES];

        compose(&vram, &vc1, &dac, &mut pixels);

        assert_eq!(pixel_at(&pixels, 0, 0), [0, 0, 0, 0xff]);
    }

    /// Uploads a solid cursor bitmap and places it at the screen origin.
    fn upload_cursor(vc1: &mut Vc1, fill: u8) {
        vc1.write(Vc1Selector::AddressHigh, 0x30);
        vc1.write(Vc1Selector::AddressLow, 0x00);
        for _ in 0..256 {
            vc1.write(Vc1Selector::Sram, fill);
        }
        // Point the generator at the bitmap and park it at the origin.
        for (address, value) in [(0x20_u16, 0x3000_u16), (0x22, 140), (0x24, 39)] {
            vc1.write(Vc1Selector::AddressHigh, (address >> 8) as u8);
            vc1.write(Vc1Selector::AddressLow, address as u8);
            vc1.write(Vc1Selector::Control, (value >> 8) as u8);
            vc1.write(Vc1Selector::Control, value as u8);
        }
    }

    #[test]
    fn an_enabled_cursor_paints_its_foreground_over_the_image() {
        let vram = Vram::new();
        let mut vc1 = Vc1::new();
        let mut dac = Bt479::new();
        select_bank(&mut dac, 3);
        write_palette(&mut dac, 1, [0xff, 0, 0]);
        // A bitmap of 0x55 selects color one for every cursor pixel.
        upload_cursor(&mut vc1, 0x55);
        vc1.write(Vc1Selector::SystemControl, SYS_CTRL_CURSOR_DISPLAY);
        let mut pixels = vec![0; FRAME_BYTES];

        compose(&vram, &vc1, &dac, &mut pixels);

        assert_eq!(pixel_at(&pixels, 0, 0), [0xff, 0, 0, 0xff]);
        assert_eq!(pixel_at(&pixels, 31, 31), [0xff, 0, 0, 0xff]);
        assert_eq!(pixel_at(&pixels, 32, 0), [0, 0, 0, 0xff]);
    }

    #[test]
    fn transparent_cursor_pixels_keep_the_underlying_image() {
        let mut vram = Vram::new();
        let mut vc1 = Vc1::new();
        let mut dac = Bt479::new();
        write_palette(&mut dac, 7, [0x40, 0x50, 0x60]);
        select_bank(&mut dac, 3);
        write_palette(&mut dac, 1, [0xff, 0, 0]);
        vram.write_masked(PlaneGroup::Pixel, 0, 0, 7, 0xff);
        // A cleared bitmap selects the transparent cursor value everywhere.
        upload_cursor(&mut vc1, 0x00);
        vc1.write(Vc1Selector::SystemControl, SYS_CTRL_CURSOR_DISPLAY);
        let mut pixels = vec![0; FRAME_BYTES];

        compose(&vram, &vc1, &dac, &mut pixels);

        assert_eq!(pixel_at(&pixels, 0, 0), [0x40, 0x50, 0x60, 0xff]);
    }
}
