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

/// Bytes occupied by one of the two cursor bit planes.
const CURSOR_PLANE_BYTES: usize = 128;

/// Bytes occupied by one row in a cursor bit plane.
const CURSOR_ROW_BYTES: usize = 4;

/// First palette entry reserved for the VC1 popup-plane submap.
const OVERLAY_PALETTE_BASE: u16 = 0x310;

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

    let mut identifiers = [0; DISPLAY_WIDTH as usize];

    for y in 0..DISPLAY_HEIGHT {
        vc1.fill_display_identifiers(y, &mut identifiers);
        let source = vram.pixel_row(y);
        let overlay = vram.overlay_row(y);
        let row = (y * DISPLAY_WIDTH) as usize * 4;
        for x in 0..DISPLAY_WIDTH {
            let overlay = overlay[x as usize];
            let index = if overlay == 0 {
                color_index_base(vc1.display_mode(identifiers[x as usize]))
                    | u16::from(source[x as usize])
            } else {
                OVERLAY_PALETTE_BASE | u16::from(overlay)
            };
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

/// Returns the palette map bits contributed by one display mode.
///
/// The XMAP mode stores the four-bit map number in bits five through two.
/// Its lower two bits become the high bits of the Bt479's ten-bit palette
/// address. The common `0x0300` prefix does not select the RGB map; Xsgi adds
/// it to both color-index and RGB display modes.
const fn color_index_base(mode: u16) -> u16 {
    ((mode >> 2) & 0x03) << 8
}

/// Draws the hardware cursor over the composed image.
fn compose_cursor(vc1: &Vc1, dac: &Bt479, pixels: &mut [u8]) {
    let Some(cursor) = vc1.cursor() else {
        return;
    };

    let foreground = dac.color(cursor.palette_base | 1);
    let background = dac.color(cursor.palette_base | 2);

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

            let byte = row as usize * CURSOR_ROW_BYTES + column as usize / 8;
            let shift = 7 - (column % 8);
            let foreground_bit = (cursor.bitmap[byte] >> shift) & 1;
            let background_bit = (cursor.bitmap[CURSOR_PLANE_BYTES + byte] >> shift) & 1;
            let color = match foreground_bit | (background_bit << 1) {
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
    use super::{CURSOR_PLANE_BYTES, FRAME_BYTES, compose, compose_blank};

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
    fn the_xmap_mode_selects_the_displayed_palette_bank() {
        let mut vram = Vram::new();
        let mut vc1 = Vc1::new();
        let mut dac = Bt479::new();
        write_palette(&mut dac, 3, [1, 1, 1]);
        select_bank(&mut dac, 2);
        write_palette(&mut dac, 3, [2, 2, 2]);
        vc1.write(Vc1Selector::AddressHigh, 0);
        vc1.write(Vc1Selector::AddressLow, 0);
        vc1.write(Vc1Selector::XmapMode, 0x03);
        vc1.write(Vc1Selector::XmapMode, 0x08);
        vram.write_masked(PlaneGroup::Pixel, 0, 0, 3, 0xff);
        let mut pixels = vec![0; FRAME_BYTES];

        compose(&vram, &vc1, &dac, &mut pixels);

        assert_eq!(pixel_at(&pixels, 0, 0), [2, 2, 2, 0xff]);
    }

    #[test]
    fn the_common_xmap_prefix_does_not_override_the_palette_map() {
        let mut vram = Vram::new();
        let mut vc1 = Vc1::new();
        let mut dac = Bt479::new();
        write_palette(&mut dac, 3, [1, 1, 1]);
        select_bank(&mut dac, 2);
        write_palette(&mut dac, 3, [2, 2, 2]);
        vc1.write(Vc1Selector::AddressHigh, 0);
        vc1.write(Vc1Selector::AddressLow, 0);
        vc1.write(Vc1Selector::XmapMode, 0x03);
        vc1.write(Vc1Selector::XmapMode, 0x00);
        vram.write_masked(PlaneGroup::Pixel, 0, 0, 3, 0xff);
        let mut pixels = vec![0; FRAME_BYTES];

        compose(&vram, &vc1, &dac, &mut pixels);

        assert_eq!(pixel_at(&pixels, 0, 0), [1, 1, 1, 0xff]);
    }

    #[test]
    fn display_identifiers_select_palette_banks_across_one_scan_line() {
        let mut vram = Vram::new();
        let mut vc1 = Vc1::new();
        let mut dac = Bt479::new();
        write_palette(&mut dac, 3, [1, 1, 1]);
        select_bank(&mut dac, 2);
        write_palette(&mut dac, 3, [2, 2, 2]);
        vc1.write(Vc1Selector::AddressHigh, 0);
        vc1.write(Vc1Selector::AddressLow, 0);
        for byte in [0x03, 0x00, 0x03, 0x08] {
            vc1.write(Vc1Selector::XmapMode, byte);
        }
        vc1.write(Vc1Selector::AddressHigh, 0x40);
        vc1.write(Vc1Selector::AddressLow, 0x00);
        for byte in [0x48, 0x00] {
            vc1.write(Vc1Selector::Sram, byte);
        }
        vc1.write(Vc1Selector::AddressHigh, 0x48);
        vc1.write(Vc1Selector::AddressLow, 0x00);
        for byte in [0x00, 0x02, 0x00, 0x00, 0x0c, 0x81] {
            vc1.write(Vc1Selector::Sram, byte);
        }
        write_control_word(&mut vc1, 0x40, 0x4000);
        vc1.write(Vc1Selector::SystemControl, 1 << 3);
        vram.write_masked(PlaneGroup::Pixel, 50, 0, 3, 0xff);
        vram.write_masked(PlaneGroup::Pixel, 150, 0, 3, 0xff);
        let mut pixels = vec![0; FRAME_BYTES];

        compose(&vram, &vc1, &dac, &mut pixels);

        assert_eq!(pixel_at(&pixels, 50, 0), [1, 1, 1, 0xff]);
        assert_eq!(pixel_at(&pixels, 150, 0), [2, 2, 2, 0xff]);
    }

    #[test]
    fn a_nonzero_overlay_pixel_selects_the_popup_submap() {
        let mut vram = Vram::new();
        let vc1 = Vc1::new();
        let mut dac = Bt479::new();
        write_palette(&mut dac, 7, [1, 1, 1]);
        select_bank(&mut dac, 3);
        write_palette(&mut dac, 0x12, [2, 2, 2]);
        vram.write_masked(PlaneGroup::Pixel, 4, 5, 7, 0xff);
        vram.write_masked(PlaneGroup::Overlay, 4, 5, 2, 0xff);
        let mut pixels = vec![0; FRAME_BYTES];

        compose(&vram, &vc1, &dac, &mut pixels);

        assert_eq!(pixel_at(&pixels, 4, 5), [2, 2, 2, 0xff]);
        assert_eq!(pixel_at(&pixels, 5, 5), [0, 0, 0, 0xff]);
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
        write_palette(&mut dac, 0x21, [0xff, 0, 0]);
        upload_cursor(&mut vc1, 0xff, 0x00);
        let mut pixels = vec![0; FRAME_BYTES];

        compose(&vram, &vc1, &dac, &mut pixels);

        assert_eq!(pixel_at(&pixels, 0, 0), [0, 0, 0, 0xff]);
    }

    /// Uploads cursor bit planes and places the cursor at the screen origin.
    fn upload_cursor(vc1: &mut Vc1, foreground: u8, background: u8) {
        vc1.write(Vc1Selector::AddressHigh, 0x30);
        vc1.write(Vc1Selector::AddressLow, 0x00);
        for fill in [foreground, background] {
            for _ in 0..CURSOR_PLANE_BYTES {
                vc1.write(Vc1Selector::Sram, fill);
            }
        }
        // Point the generator at the bitmap and park it at the origin.
        for (address, value) in [
            (0x20_u16, 0x3000_u16),
            (0x22, 140),
            (0x24, 39),
            (0x26, 0xc800),
        ] {
            write_control_word(vc1, address, value);
        }
    }

    /// Writes one big-endian VC1 control word.
    fn write_control_word(vc1: &mut Vc1, address: u16, value: u16) {
        vc1.write(Vc1Selector::AddressHigh, (address >> 8) as u8);
        vc1.write(Vc1Selector::AddressLow, address as u8);
        vc1.write(Vc1Selector::Control, (value >> 8) as u8);
        vc1.write(Vc1Selector::Control, value as u8);
    }

    #[test]
    fn an_enabled_cursor_paints_its_foreground_over_the_image() {
        let vram = Vram::new();
        let mut vc1 = Vc1::new();
        let mut dac = Bt479::new();
        select_bank(&mut dac, 3);
        write_palette(&mut dac, 0x21, [0xff, 0, 0]);
        upload_cursor(&mut vc1, 0xff, 0x00);
        vc1.write(Vc1Selector::SystemControl, SYS_CTRL_CURSOR_DISPLAY);
        let mut pixels = vec![0; FRAME_BYTES];

        compose(&vram, &vc1, &dac, &mut pixels);

        assert_eq!(pixel_at(&pixels, 0, 0), [0xff, 0, 0, 0xff]);
        assert_eq!(pixel_at(&pixels, 31, 31), [0xff, 0, 0, 0xff]);
        assert_eq!(pixel_at(&pixels, 32, 0), [0, 0, 0, 0xff]);
    }

    #[test]
    fn the_second_cursor_plane_selects_the_background_color() {
        let vram = Vram::new();
        let mut vc1 = Vc1::new();
        let mut dac = Bt479::new();
        select_bank(&mut dac, 3);
        write_palette(&mut dac, 0x22, [0, 0, 0xff]);
        upload_cursor(&mut vc1, 0x00, 0xff);
        vc1.write(Vc1Selector::SystemControl, SYS_CTRL_CURSOR_DISPLAY);
        let mut pixels = vec![0; FRAME_BYTES];

        compose(&vram, &vc1, &dac, &mut pixels);

        assert_eq!(pixel_at(&pixels, 0, 0), [0, 0, 0xff, 0xff]);
        assert_eq!(pixel_at(&pixels, 31, 31), [0, 0, 0xff, 0xff]);
    }

    #[test]
    fn cursor_mode_selects_the_palette_submap() {
        let vram = Vram::new();
        let mut vc1 = Vc1::new();
        let mut dac = Bt479::new();
        select_bank(&mut dac, 3);
        write_palette(&mut dac, 0x01, [0, 0xff, 0]);
        write_palette(&mut dac, 0x21, [0xff, 0, 0]);
        upload_cursor(&mut vc1, 0xff, 0x00);
        write_control_word(&mut vc1, 0x26, 0xc000);
        vc1.write(Vc1Selector::SystemControl, SYS_CTRL_CURSOR_DISPLAY);
        let mut pixels = vec![0; FRAME_BYTES];

        compose(&vram, &vc1, &dac, &mut pixels);

        assert_eq!(pixel_at(&pixels, 0, 0), [0, 0xff, 0, 0xff]);
    }

    #[test]
    fn transparent_cursor_pixels_keep_the_underlying_image() {
        let mut vram = Vram::new();
        let mut vc1 = Vc1::new();
        let mut dac = Bt479::new();
        write_palette(&mut dac, 7, [0x40, 0x50, 0x60]);
        select_bank(&mut dac, 3);
        write_palette(&mut dac, 0x21, [0xff, 0, 0]);
        vram.write_masked(PlaneGroup::Pixel, 0, 0, 7, 0xff);
        upload_cursor(&mut vc1, 0x00, 0x00);
        vc1.write(Vc1Selector::SystemControl, SYS_CTRL_CURSOR_DISPLAY);
        let mut pixels = vec![0; FRAME_BYTES];

        compose(&vram, &vc1, &dac, &mut pixels);

        assert_eq!(pixel_at(&pixels, 0, 0), [0x40, 0x50, 0x60, 0xff]);
    }
}
