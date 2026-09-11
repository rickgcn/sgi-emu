//! LG1 frame buffer storage for the pixel, overlay, and CID planes.

use serde::{Deserialize, Serialize};

/// Frame buffer width in pixels.
pub(super) const WIDTH: u32 = 1024;

/// Frame buffer height in pixels, including the off-screen rows.
///
/// The Indigo technical report documents 816 rows of storage for a display
/// that shows 768 of them. The remaining 48 rows stay addressable so guest
/// drawing into off-screen space is preserved.
pub(super) const HEIGHT: u32 = 816;

const PIXEL_COUNT: usize = (WIDTH * HEIGHT) as usize;

/// Significant bits retained by the overlay and CID planes.
const AUXILIARY_MASK: u8 = 0x03;

/// The drawing plane group selected by the `aux2` register.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PlaneGroup {
    /// No plane group is selected and writes are discarded.
    None,
    /// The eight-bit pixel plane.
    Pixel,
    /// The two-bit overlay plane.
    Overlay,
    /// The two-bit clipping identifier plane.
    Cid,
}

impl PlaneGroup {
    /// Decodes the plane group held in `aux2` bits 30:29.
    pub(super) const fn from_aux2(aux2: u32) -> Self {
        match (aux2 >> 29) & 0x03 {
            1 => Self::Pixel,
            2 => Self::Overlay,
            3 => Self::Cid,
            _ => Self::None,
        }
    }

    /// Returns the mask of bits the plane group can hold.
    const fn value_mask(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Pixel => 0xff,
            Self::Overlay | Self::Cid => AUXILIARY_MASK,
        }
    }
}

/// The three independently addressed LG1 frame buffer planes.
#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Vram {
    pixel: Box<[u8]>,
    overlay: Box<[u8]>,
    cid: Box<[u8]>,
}

impl Vram {
    /// Creates cleared frame buffer planes.
    pub(super) fn new() -> Self {
        Self {
            pixel: vec![0; PIXEL_COUNT].into_boxed_slice(),
            overlay: vec![0; PIXEL_COUNT].into_boxed_slice(),
            cid: vec![0; PIXEL_COUNT].into_boxed_slice(),
        }
    }

    /// Clears every plane, including the off-screen rows.
    pub(super) fn reset(&mut self) {
        self.pixel.fill(0);
        self.overlay.fill(0);
        self.cid.fill(0);
    }

    /// Returns the pixel plane row for one displayed scan line.
    pub(super) fn pixel_row(&self, y: u32) -> &[u8] {
        let start = row_start(y);
        &self.pixel[start..start + WIDTH as usize]
    }

    /// Returns the overlay plane row for one displayed scan line.
    pub(super) fn overlay_row(&self, y: u32) -> &[u8] {
        let start = row_start(y);
        &self.overlay[start..start + WIDTH as usize]
    }

    /// Reads one plane value, returning zero outside the stored area.
    pub(super) fn read(&self, group: PlaneGroup, x: u32, y: u32) -> u8 {
        let Some(index) = index(x, y) else {
            return 0;
        };
        match group {
            PlaneGroup::None => 0,
            PlaneGroup::Pixel => self.pixel[index],
            PlaneGroup::Overlay => self.overlay[index],
            PlaneGroup::Cid => self.cid[index],
        }
    }

    /// Applies one masked plane write, discarding coordinates outside storage.
    ///
    /// `value` supplies the already combined source and destination result;
    /// only the bits selected by `write_mask` replace stored data.
    pub(super) fn write_masked(
        &mut self,
        group: PlaneGroup,
        x: u32,
        y: u32,
        value: u8,
        write_mask: u8,
    ) {
        let Some(index) = index(x, y) else {
            return;
        };
        let mask = write_mask & group.value_mask();
        if mask == 0 {
            return;
        }
        let plane = match group {
            PlaneGroup::None => return,
            PlaneGroup::Pixel => &mut self.pixel[index],
            PlaneGroup::Overlay => &mut self.overlay[index],
            PlaneGroup::Cid => &mut self.cid[index],
        };
        *plane = (*plane & !mask) | (value & mask);
    }
}

/// Returns the storage index for one coordinate inside the frame buffer.
const fn index(x: u32, y: u32) -> Option<usize> {
    if x >= WIDTH || y >= HEIGHT {
        return None;
    }
    Some((y * WIDTH + x) as usize)
}

/// Returns the storage index of the first pixel in one row.
///
/// # Panics
///
/// Panics when `y` lies outside the stored rows.
const fn row_start(y: u32) -> usize {
    assert!(y < HEIGHT, "scan line must lie inside frame buffer storage");
    (y * WIDTH) as usize
}

#[cfg(test)]
mod tests {
    use super::{HEIGHT, PlaneGroup, Vram, WIDTH};

    #[test]
    fn aux2_selects_the_documented_plane_groups() {
        assert_eq!(PlaneGroup::from_aux2(0), PlaneGroup::None);
        assert_eq!(PlaneGroup::from_aux2(0x2000_0000), PlaneGroup::Pixel);
        assert_eq!(PlaneGroup::from_aux2(0x4000_0000), PlaneGroup::Overlay);
        assert_eq!(PlaneGroup::from_aux2(0x6000_0000), PlaneGroup::Cid);
    }

    #[test]
    fn auxiliary_planes_retain_only_two_bits() {
        let mut vram = Vram::new();

        vram.write_masked(PlaneGroup::Pixel, 1, 1, 0xa5, 0xff);
        vram.write_masked(PlaneGroup::Overlay, 1, 1, 0xa5, 0xff);
        vram.write_masked(PlaneGroup::Cid, 1, 1, 0xa5, 0xff);

        assert_eq!(vram.read(PlaneGroup::Pixel, 1, 1), 0xa5);
        assert_eq!(vram.read(PlaneGroup::Overlay, 1, 1), 0x01);
        assert_eq!(vram.read(PlaneGroup::Cid, 1, 1), 0x01);
    }

    #[test]
    fn write_mask_preserves_unselected_bits() {
        let mut vram = Vram::new();
        vram.write_masked(PlaneGroup::Pixel, 2, 3, 0xff, 0xff);

        vram.write_masked(PlaneGroup::Pixel, 2, 3, 0x00, 0x0f);

        assert_eq!(vram.read(PlaneGroup::Pixel, 2, 3), 0xf0);
    }

    #[test]
    fn the_no_plane_group_discards_writes() {
        let mut vram = Vram::new();

        vram.write_masked(PlaneGroup::None, 4, 5, 0xff, 0xff);

        assert_eq!(vram.read(PlaneGroup::Pixel, 4, 5), 0);
        assert_eq!(vram.read(PlaneGroup::Overlay, 4, 5), 0);
        assert_eq!(vram.read(PlaneGroup::Cid, 4, 5), 0);
    }

    #[test]
    fn off_screen_rows_are_addressable_but_outside_storage_is_discarded() {
        let mut vram = Vram::new();

        vram.write_masked(PlaneGroup::Pixel, 0, HEIGHT - 1, 0x5a, 0xff);
        vram.write_masked(PlaneGroup::Pixel, WIDTH, 0, 0x5a, 0xff);
        vram.write_masked(PlaneGroup::Pixel, 0, HEIGHT, 0x5a, 0xff);

        assert_eq!(vram.read(PlaneGroup::Pixel, 0, HEIGHT - 1), 0x5a);
        assert_eq!(vram.read(PlaneGroup::Pixel, WIDTH, 0), 0);
        assert_eq!(vram.read(PlaneGroup::Pixel, 0, HEIGHT), 0);
    }

    #[test]
    fn a_row_exposes_one_displayed_scan_line_of_the_pixel_plane() {
        let mut vram = Vram::new();
        vram.write_masked(PlaneGroup::Pixel, 7, 9, 0x33, 0xff);
        vram.write_masked(PlaneGroup::Overlay, 7, 9, 0x02, 0xff);

        assert_eq!(vram.pixel_row(9).len(), WIDTH as usize);
        assert_eq!(vram.pixel_row(9)[7], 0x33);
        assert_eq!(vram.pixel_row(8)[7], 0);
        assert_eq!(vram.overlay_row(9)[7], 0x02);
        assert_eq!(vram.overlay_row(8)[7], 0);
    }

    #[test]
    fn reset_clears_every_plane() {
        let mut vram = Vram::new();
        vram.write_masked(PlaneGroup::Pixel, 1, 1, 0xff, 0xff);
        vram.write_masked(PlaneGroup::Overlay, 1, 1, 0xff, 0xff);
        vram.write_masked(PlaneGroup::Cid, 1, 1, 0xff, 0xff);

        vram.reset();

        assert_eq!(vram.read(PlaneGroup::Pixel, 1, 1), 0);
        assert_eq!(vram.read(PlaneGroup::Overlay, 1, 1), 0);
        assert_eq!(vram.read(PlaneGroup::Cid, 1, 1), 0);
    }
}
