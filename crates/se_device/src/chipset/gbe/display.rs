//! Pure display-pipeline decoding and frame conversion helpers.

use super::registers::GbeRegisters;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(super) enum PlaneDepth {
    Eight,
    Sixteen,
    ThirtyTwo,
}

impl PlaneDepth {
    pub fn from_frame_register(value: u32) -> Self {
        match (value >> 13) & 3 {
            1 => Self::Sixteen,
            2 => Self::ThirtyTwo,
            _ => Self::Eight,
        }
    }

    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Eight => 1,
            Self::Sixteen => 2,
            Self::ThirtyTwo => 4,
        }
    }

    pub const fn tile_width(self) -> usize {
        512 / self.bytes_per_pixel()
    }
}

pub(super) fn reorder_cgi_pixel_words(data: &[u8]) -> Vec<u8> {
    let mut reordered = Vec::with_capacity(data.len());
    for block in data.chunks(32) {
        for word in (0..block.len().div_ceil(4)).rev() {
            let start = word * 4;
            let end = (start + 4).min(block.len());
            reordered.extend_from_slice(&block[start..end]);
        }
    }
    reordered
}

pub(super) fn decode_raw_pixels(data: &[u8], depth: PlaneDepth) -> Vec<u32> {
    let reordered = reorder_cgi_pixel_words(data);
    match depth {
        PlaneDepth::Eight => reordered.into_iter().map(u32::from).collect(),
        PlaneDepth::Sixteen => reordered
            .chunks_exact(2)
            .map(|bytes| u32::from(u16::from_be_bytes([bytes[0], bytes[1]])))
            .collect(),
        PlaneDepth::ThirtyTwo => reordered
            .chunks_exact(4)
            .map(|bytes| u32::from_be_bytes(bytes.try_into().expect("four-byte pixel")))
            .collect(),
    }
}

pub(super) fn visible_dimensions(registers: &GbeRegisters) -> (usize, usize) {
    let depth = PlaneDepth::from_frame_register(registers.frame[0]);
    let tiles = usize::try_from((registers.frame[0] >> 5) & 0xff).unwrap_or(0);
    let right = usize::try_from(registers.frame[0] & 0x1f).unwrap_or(0);
    let width = tiles.saturating_mul(depth.tile_width()).saturating_add(
        right
            .saturating_mul(32)
            .checked_div(depth.bytes_per_pixel())
            .unwrap_or(0),
    );
    let height = usize::try_from(registers.frame[1] >> 16).unwrap_or(0);
    (width.min(4_096), height.min(4_096))
}

pub(super) fn color_from_normal(
    registers: &GbeRegisters,
    raw: u32,
    depth: PlaneDepth,
    did: u8,
) -> ([u8; 3], u8) {
    let wid = registers.wid[usize::from(did & 0x1f)];
    let mode = (wid >> 2) & 7;
    let buffer = wid & 3;
    let color_map = (wid >> 5) & 0x1f;
    let gamma_enabled = wid & (1 << 10) == 0;
    let aux = ((wid >> 11) & 3) as u8;
    let selected = select_buffer(raw, depth, mode, buffer);
    let rgb = match mode {
        0 => {
            let index = ((color_map & 0xf) << 8) | (selected & 0xff);
            packed_rgb(registers.color_map[index as usize])
        }
        1 => packed_rgb(registers.color_map[(selected & 0xfff) as usize]),
        2 => [
            expand((selected >> 5) & 7, 3),
            expand((selected >> 2) & 7, 3),
            expand(selected & 3, 2),
        ],
        3 => [
            expand((selected >> 12) & 0xf, 4),
            expand((selected >> 8) & 0xf, 4),
            expand((selected >> 4) & 0xf, 4),
        ],
        4 | 6 => [
            expand((selected >> 10) & 0x1f, 5),
            expand((selected >> 5) & 0x1f, 5),
            expand(selected & 0x1f, 5),
        ],
        5 => [
            ((selected >> 24) & 0xff) as u8,
            ((selected >> 16) & 0xff) as u8,
            ((selected >> 8) & 0xff) as u8,
        ],
        _ => [0, 0, 0],
    };
    let rgb = if matches!(mode, 2..=5) && color_map != 0 {
        component_color_map(registers, color_map, rgb)
    } else {
        rgb
    };
    (
        if gamma_enabled {
            apply_gamma(registers, rgb)
        } else {
            rgb
        },
        aux,
    )
}

pub(super) fn color_from_overlay(registers: &GbeRegisters, value: u8) -> Option<[u8; 3]> {
    if value == 0 {
        return None;
    }
    Some(apply_gamma(
        registers,
        packed_rgb(registers.color_map[4_352 + usize::from(value)]),
    ))
}

pub(super) fn cursor_color(
    registers: &GbeRegisters,
    visible_x: usize,
    visible_y: usize,
) -> Option<[u8; 3]> {
    if registers.cursor[1] & 1 == 0 {
        return None;
    }
    let cursor_x = usize::try_from(registers.cursor[0] & 0xffff).ok()?;
    let cursor_y = usize::try_from(registers.cursor[0] >> 16).ok()?;
    if registers.cursor[1] & 2 != 0 {
        return (visible_x == cursor_x || visible_y == cursor_y)
            .then(|| packed_rgb(registers.cursor[2]));
    }
    let x = visible_x.saturating_add(31).checked_sub(cursor_x)?;
    let y = visible_y.saturating_add(31).checked_sub(cursor_y)?;
    if x >= 32 || y >= 32 {
        return None;
    }
    let word = registers.cursor_glyph[y * 2 + x / 16];
    let shift = 30 - (x % 16) * 2;
    let index = ((word >> shift) & 3) as usize;
    if index == 0 {
        None
    } else {
        Some(packed_rgb(registers.cursor[index + 1]))
    }
}

pub(super) fn did_for_pixel(frame_table: &[u8], line_table: &[u8], y: u16, x: u16) -> u8 {
    let mut offset = 0_usize;
    let mut found = false;
    for entry in frame_table.chunks_exact(4) {
        let value = u32::from_be_bytes(entry.try_into().expect("four-byte DID frame entry"));
        let y_end = (value >> 21) as u16;
        if y <= y_end {
            offset = usize::try_from((value >> 13) & 0xff).unwrap_or(0) * 2;
            found = true;
            break;
        }
    }
    if !found {
        return 0;
    }
    for entry in line_table.get(offset..).unwrap_or_default().chunks_exact(2) {
        let value = u16::from_be_bytes([entry[0], entry[1]]);
        if x <= value >> 5 {
            return (value & 0x1f) as u8;
        }
    }
    0
}

pub(super) fn did_block_for_line(frame_table: &[u8], y: u16) -> Option<u8> {
    frame_table.chunks_exact(4).find_map(|entry| {
        let value = u32::from_be_bytes(entry.try_into().expect("four-byte DID frame entry"));
        (y <= (value >> 21) as u16).then_some(((value >> 6) & 0x7f) as u8)
    })
}

pub(super) fn area_resample_rgba(
    source: &[u8],
    source_width: usize,
    source_height: usize,
    target_width: usize,
    target_height: usize,
) -> Vec<u8> {
    if source_width == 0 || source_height == 0 || target_width == 0 || target_height == 0 {
        return Vec::new();
    }
    let mut target = vec![0; target_width * target_height * 4];
    for target_y in 0..target_height {
        let source_y =
            ((2 * target_y + 1) * source_height / (2 * target_height)).min(source_height - 1);
        for target_x in 0..target_width {
            let source_x =
                ((2 * target_x + 1) * source_width / (2 * target_width)).min(source_width - 1);
            let source_offset = (source_y * source_width + source_x) * 4;
            let target_offset = (target_y * target_width + target_x) * 4;
            target[target_offset..target_offset + 4]
                .copy_from_slice(&source[source_offset..source_offset + 4]);
        }
    }
    target
}

pub(super) fn filter_fullscreen_rgba(
    source: &[u8],
    source_width: usize,
    source_height: usize,
    pal: bool,
    flicker: bool,
    odd_field: bool,
) -> (usize, usize, Vec<u8>) {
    let target_width = if pal { 768 } else { 640 };
    let full_height = if pal { 576 } else { 480 };
    let horizontal = if source_width == 1_280 {
        source
            .chunks_exact(source_width * 4)
            .flat_map(|line| filter_line_rgba(line, pal))
            .collect::<Vec<_>>()
    } else {
        area_resample_rgba(
            source,
            source_width,
            source_height,
            target_width,
            source_height,
        )
    };
    let vertical = if source_height == 960 {
        filter_rows_rgba(&horizontal, target_width, source_height, pal, full_height)
    } else {
        area_resample_rgba(
            &horizontal,
            target_width,
            source_height,
            target_width,
            full_height,
        )
    };
    let field_height = full_height / 2;
    let parity = usize::from(odd_field);
    let mut field = vec![0; target_width * field_height * 4];
    for output_y in 0..field_height {
        let center = (output_y * 2 + parity).min(full_height - 1);
        let previous = center.saturating_sub(1);
        let next = (center + 1).min(full_height - 1);
        for x in 0..target_width {
            let output = (output_y * target_width + x) * 4;
            let center_offset = (center * target_width + x) * 4;
            if flicker {
                let previous_offset = (previous * target_width + x) * 4;
                let next_offset = (next * target_width + x) * 4;
                for channel in 0..4 {
                    field[output + channel] = ((u16::from(vertical[previous_offset + channel])
                        + 2 * u16::from(vertical[center_offset + channel])
                        + u16::from(vertical[next_offset + channel])
                        + 2)
                        / 4) as u8;
                }
            } else {
                field[output..output + 4]
                    .copy_from_slice(&vertical[center_offset..center_offset + 4]);
            }
        }
    }
    (target_width, field_height, field)
}

fn filter_line_rgba(source: &[u8], pal: bool) -> Vec<u8> {
    if pal {
        let mut output = Vec::with_capacity(768 * 4);
        for group in source.chunks_exact(5 * 4) {
            append_weighted_pixel(&mut output, group, &[(0, 2), (1, 1)], 3);
            append_weighted_pixel(&mut output, group, &[(1, 1), (2, 2), (3, 1)], 4);
            append_weighted_pixel(&mut output, group, &[(3, 1), (4, 2)], 3);
        }
        output
    } else {
        let pixels = source.len() / 4;
        let mut output = Vec::with_capacity(640 * 4);
        for center in (0..pixels).step_by(2) {
            let previous = center.saturating_sub(1);
            let next = (center + 1).min(pixels - 1);
            append_weighted_pixel(
                &mut output,
                source,
                &[(previous, 1), (center, 2), (next, 1)],
                4,
            );
        }
        output
    }
}

fn filter_rows_rgba(
    source: &[u8],
    width: usize,
    source_height: usize,
    pal: bool,
    target_height: usize,
) -> Vec<u8> {
    let mut output = vec![0; width * target_height * 4];
    for x in 0..width {
        let mut column = Vec::with_capacity(source_height * 4);
        for y in 0..source_height {
            let offset = (y * width + x) * 4;
            column.extend_from_slice(&source[offset..offset + 4]);
        }
        let filtered = filter_line_rgba(&column, pal);
        for y in 0..target_height {
            let source_offset = y * 4;
            let target_offset = (y * width + x) * 4;
            output[target_offset..target_offset + 4]
                .copy_from_slice(&filtered[source_offset..source_offset + 4]);
        }
    }
    output
}

fn append_weighted_pixel(
    output: &mut Vec<u8>,
    source: &[u8],
    weights: &[(usize, u16)],
    denominator: u16,
) {
    for channel in 0..4 {
        let sum = weights.iter().fold(0_u16, |sum, (pixel, weight)| {
            sum + u16::from(source[pixel * 4 + channel]) * weight
        });
        output.push(((sum + denominator / 2) / denominator) as u8);
    }
}

fn select_buffer(raw: u32, depth: PlaneDepth, mode: u32, buffer: u32) -> u32 {
    match (depth, mode, buffer) {
        (PlaneDepth::Sixteen, 0, 1) => raw & 0xff,
        (PlaneDepth::Sixteen, 0, 2) => raw >> 8,
        (PlaneDepth::ThirtyTwo, 0..=4 | 6, 1) => raw & 0xffff,
        (PlaneDepth::ThirtyTwo, 0..=4 | 6, 2) => raw >> 16,
        _ => raw,
    }
}

fn packed_rgb(value: u32) -> [u8; 3] {
    [
        ((value >> 24) & 0xff) as u8,
        ((value >> 16) & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
    ]
}

fn apply_gamma(registers: &GbeRegisters, rgb: [u8; 3]) -> [u8; 3] {
    [
        ((registers.gamma_map[usize::from(rgb[0])] >> 24) & 0xff) as u8,
        ((registers.gamma_map[usize::from(rgb[1])] >> 16) & 0xff) as u8,
        ((registers.gamma_map[usize::from(rgb[2])] >> 8) & 0xff) as u8,
    ]
}

fn component_color_map(registers: &GbeRegisters, map: u32, rgb: [u8; 3]) -> [u8; 3] {
    let index = |component: u8| {
        usize::try_from((map << 8) | u32::from(component))
            .ok()
            .and_then(|index| registers.color_map.get(index))
            .copied()
            .unwrap_or(0)
    };
    [
        (index(rgb[0]) >> 24) as u8,
        (index(rgb[1]) >> 16) as u8,
        (index(rgb[2]) >> 8) as u8,
    ]
}

fn expand(value: u32, bits: u32) -> u8 {
    match bits {
        2 => ((value << 6) | (value << 4) | (value << 2) | value) as u8,
        3 => ((value << 5) | (value << 2) | (value >> 1)) as u8,
        4 => ((value << 4) | value) as u8,
        5 => ((value << 3) | (value >> 2)) as u8,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cgi_pixel_words_are_consumed_from_high_address_to_low_address() {
        let source = (0_u8..32).collect::<Vec<_>>();
        let reordered = reorder_cgi_pixel_words(&source);
        assert_eq!(&reordered[0..4], &[28, 29, 30, 31]);
        assert_eq!(&reordered[28..32], &[0, 1, 2, 3]);
    }

    #[test]
    fn all_frame_depths_decode_big_endian_pixels() {
        let source = (0_u8..32).collect::<Vec<_>>();
        assert_eq!(decode_raw_pixels(&source, PlaneDepth::Eight)[0], 28);
        assert_eq!(decode_raw_pixels(&source, PlaneDepth::Sixteen)[0], 0x1c1d);
        assert_eq!(
            decode_raw_pixels(&source, PlaneDepth::ThirtyTwo)[0],
            0x1c1d_1e1f
        );
    }

    #[test]
    fn ntsc_horizontal_filter_uses_one_quarter_one_half_one_quarter() {
        let source = (0_u8..10)
            .flat_map(|value| [value * 10, 0, 0, 0xff])
            .collect::<Vec<_>>();
        let filtered = filter_line_rgba(&source, false);
        assert_eq!(filtered[0], 3);
        assert_eq!(filtered[4], 20);
        assert_eq!(filtered[8], 40);
    }

    #[test]
    fn pal_horizontal_filter_repeats_the_three_outputs_per_five_inputs() {
        let source = (0_u8..10)
            .flat_map(|value| [value * 12, 0, 0, 0xff])
            .collect::<Vec<_>>();
        let filtered = filter_line_rgba(&source, true);
        let red = filtered
            .chunks_exact(4)
            .map(|pixel| pixel[0])
            .collect::<Vec<_>>();
        assert_eq!(red, [4, 24, 44, 64, 84, 104]);
    }

    #[test]
    fn did_frame_and_line_tables_decode_final_register_table_layout() {
        let frame_entry = ((7_u32 << 21) | (3 << 13) | (5 << 6)).to_be_bytes();
        assert_eq!(did_block_for_line(&frame_entry, 7), Some(5));
        let mut line = vec![0; 512];
        let entry = ((31_u16 << 5) | 9).to_be_bytes();
        line[6..8].copy_from_slice(&entry);
        assert_eq!(did_for_pixel(&frame_entry, &line, 7, 31), 9);
    }

    #[test]
    fn rgb_formats_use_documented_component_positions_and_buffer_codes() {
        let mut registers = GbeRegisters::new();
        registers.wid[0] = (5 << 2) | 3 | (1 << 10);
        assert_eq!(
            color_from_normal(&registers, 0x1234_56ff, PlaneDepth::ThirtyTwo, 0).0,
            [0x12, 0x34, 0x56]
        );
        registers.wid[0] = (3 << 2) | 3 | (1 << 10);
        assert_eq!(
            color_from_normal(&registers, 0x0000_abc0, PlaneDepth::Sixteen, 0).0,
            [0xaa, 0xbb, 0xcc]
        );
        registers.wid[0] = 1 | (1 << 10);
        registers.color_map[0x34] = 0x1020_3000;
        assert_eq!(
            color_from_normal(&registers, 0x1234, PlaneDepth::Sixteen, 0).0,
            [0x10, 0x20, 0x30]
        );

        registers.wid[0] = (1 << 2) | 3 | (1 << 10);
        registers.color_map[0xabc] = 0x4050_6000;
        assert_eq!(
            color_from_normal(&registers, 0x0abc, PlaneDepth::Sixteen, 0).0,
            [0x40, 0x50, 0x60]
        );
        registers.wid[0] = (2 << 2) | 3 | (1 << 10);
        assert_eq!(
            color_from_normal(&registers, 0xe5, PlaneDepth::Eight, 0).0,
            [0xff, 0x24, 0x55]
        );
        registers.wid[0] = (4 << 2) | 3 | (1 << 10);
        assert_eq!(
            color_from_normal(
                &registers,
                (31 << 10) | (16 << 5) | 1,
                PlaneDepth::Sixteen,
                0,
            )
            .0,
            [0xff, 0x84, 0x08]
        );
    }

    #[test]
    fn overlay_always_uses_gamma_while_cursor_always_bypasses_it() {
        let mut registers = GbeRegisters::new();
        registers.color_map[4_353] = 0x0a14_1e00;
        registers.gamma_map[10] = 0xaa00_0000;
        registers.gamma_map[20] = 0x00bb_0000;
        registers.gamma_map[30] = 0x0000_cc00;
        assert_eq!(color_from_overlay(&registers, 1), Some([0xaa, 0xbb, 0xcc]));

        registers.cursor[0] = (31 << 16) | 31;
        registers.cursor[1] = 1;
        registers.cursor[2] = 0x0a14_1e00;
        registers.cursor_glyph[0] = 1 << 30;
        assert_eq!(cursor_color(&registers, 0, 0), Some([10, 20, 30]));
    }

    #[test]
    fn cursor_position_zero_exposes_only_the_lower_right_glyph_pixel_at_origin() {
        let mut registers = GbeRegisters::new();
        registers.cursor[1] = 1;
        registers.cursor[2] = 0x4455_6600;
        registers.cursor_glyph[63] = 1;
        assert_eq!(cursor_color(&registers, 0, 0), Some([0x44, 0x55, 0x66]));
        assert_eq!(cursor_color(&registers, 1, 0), None);
    }

    #[test]
    fn fullscreen_filters_produce_square_pixel_interlaced_field_sizes() {
        let source = [0x12, 0x34, 0x56, 0xff]
            .into_iter()
            .cycle()
            .take(1_280 * 960 * 4)
            .collect::<Vec<_>>();
        for (pal, expected) in [(false, (640, 240)), (true, (768, 288))] {
            let (width, height, field) =
                filter_fullscreen_rgba(&source, 1_280, 960, pal, true, false);
            assert_eq!((width, height), expected);
            assert!(
                field
                    .chunks_exact(4)
                    .all(|pixel| pixel == [0x12, 0x34, 0x56, 0xff])
            );
        }
    }

    #[test]
    fn framebuffer_width_adds_the_right_hand_partial_tile() {
        let mut registers = GbeRegisters::new();
        registers.frame[0] = (1 << 5) | 4;
        registers.frame[1] = 480 << 16;
        assert_eq!(visible_dimensions(&registers), (640, 480));

        registers.frame[0] = (1 << 13) | (2 << 5) | 2;
        assert_eq!(visible_dimensions(&registers), (544, 480));
    }
}
