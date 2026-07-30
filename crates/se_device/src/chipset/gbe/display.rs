//! Pure display-pipeline decoding and frame conversion helpers.

use super::registers::{
    CURSOR_START_XY, DID_START_XY, GbeRegisters, VT_HPIXEN, VT_VBLANK, VT_VPIXEN, VT_XY_MAX,
};
use super::{
    align_up, interval_length, lower_endpoint, plane_width, tile_columns, triggered_counter,
    upper_endpoint,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) enum PlaneDepth {
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

#[cfg(test)]
pub(super) fn reorder_cgi_pixel_words(data: &[u8]) -> Vec<u8> {
    reordered_cgi_pixel_bytes(data).collect()
}

#[cfg(test)]
pub(crate) fn decode_raw_pixels(data: &[u8], depth: PlaneDepth) -> Vec<u32> {
    let mut output = vec![0; data.len() / depth.bytes_per_pixel()];
    let decoded = decode_raw_pixels_into(data, depth, 0, &mut output);
    output.truncate(decoded);
    output
}

pub(super) fn decode_raw_pixels_into(
    data: &[u8],
    depth: PlaneDepth,
    source_pixel: usize,
    output: &mut [u32],
) -> usize {
    let Some(byte_offset) = source_pixel.checked_mul(depth.bytes_per_pixel()) else {
        return 0;
    };
    let mut bytes = reordered_cgi_pixel_bytes(data).skip(byte_offset);
    let mut decoded = 0;
    for pixel in output {
        let value = match depth {
            PlaneDepth::Eight => {
                let Some(byte) = bytes.next() else {
                    break;
                };
                u32::from(byte)
            }
            PlaneDepth::Sixteen => {
                let (Some(high), Some(low)) = (bytes.next(), bytes.next()) else {
                    break;
                };
                u32::from(u16::from_be_bytes([high, low]))
            }
            PlaneDepth::ThirtyTwo => {
                let (Some(first), Some(second), Some(third), Some(fourth)) =
                    (bytes.next(), bytes.next(), bytes.next(), bytes.next())
                else {
                    break;
                };
                u32::from_be_bytes([first, second, third, fourth])
            }
        };
        *pixel = value;
        decoded += 1;
    }
    decoded
}

fn reordered_cgi_pixel_bytes(data: &[u8]) -> impl Iterator<Item = u8> + '_ {
    data.chunks(32).flat_map(|block| {
        (0..block.len().div_ceil(4)).rev().flat_map(move |word| {
            let start = word * 4;
            let end = (start + 4).min(block.len());
            block[start..end].iter().copied()
        })
    })
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
    let x = visible_x.checked_sub(cursor_x)?;
    let y = visible_y.checked_sub(cursor_y)?;
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

/// Per-channel gamma correction tables captured once per composed frame.
struct GammaLut {
    red: [u8; 256],
    green: [u8; 256],
    blue: [u8; 256],
}

impl GammaLut {
    fn new(registers: &GbeRegisters) -> Self {
        let mut lut = Self {
            red: [0; 256],
            green: [0; 256],
            blue: [0; 256],
        };
        for (index, value) in registers.gamma_map.iter().enumerate() {
            lut.red[index] = ((value >> 24) & 0xff) as u8;
            lut.green[index] = ((value >> 16) & 0xff) as u8;
            lut.blue[index] = ((value >> 8) & 0xff) as u8;
        }
        lut
    }

    fn apply(&self, rgb: [u8; 3]) -> [u8; 3] {
        [
            self.red[usize::from(rgb[0])],
            self.green[usize::from(rgb[1])],
            self.blue[usize::from(rgb[2])],
        ]
    }
}

/// Double-buffer extraction applied to one raw pixel before color resolution.
#[derive(Clone, Copy)]
enum BufferSelect {
    Full,
    Low8,
    High8,
    Low16,
    High16,
}

impl BufferSelect {
    fn for_wid(depth: PlaneDepth, mode: u32, buffer: u32) -> Self {
        match (depth, mode, buffer) {
            (PlaneDepth::Sixteen, 0, 1) => Self::Low8,
            (PlaneDepth::Sixteen, 0, 2) => Self::High8,
            (PlaneDepth::ThirtyTwo, 0..=4 | 6, 1) => Self::Low16,
            (PlaneDepth::ThirtyTwo, 0..=4 | 6, 2) => Self::High16,
            _ => Self::Full,
        }
    }

    fn apply(self, raw: u32) -> u32 {
        match self {
            Self::Full => raw,
            Self::Low8 => raw & 0xff,
            Self::High8 => raw >> 8,
            Self::Low16 => raw & 0xffff,
            Self::High16 => raw >> 16,
        }
    }
}

/// Precomputed per-DID color resolution for one composed frame.
struct DidColor {
    select: BufferSelect,
    kind: DidColorKind,
}

enum DidColorKind {
    Indexed8(Box<[[u8; 3]; 256]>),
    Mapped12(Box<[[u8; 3]; 4_096]>),
    Direct {
        mode: u32,
        component_map: Option<Box<ComponentMapLut>>,
        gamma: bool,
    },
    Black,
}

struct ComponentMapLut {
    red: [u8; 256],
    green: [u8; 256],
    blue: [u8; 256],
}

impl ComponentMapLut {
    fn new(registers: &GbeRegisters, map: u32) -> Self {
        let mut lut = Self {
            red: [0; 256],
            green: [0; 256],
            blue: [0; 256],
        };
        for component in 0..256_u32 {
            let index = usize::try_from((map << 8) | component).unwrap_or(usize::MAX);
            let value = registers.color_map.get(index).copied().unwrap_or(0);
            lut.red[component as usize] = (value >> 24) as u8;
            lut.green[component as usize] = (value >> 16) as u8;
            lut.blue[component as usize] = (value >> 8) as u8;
        }
        lut
    }

    fn apply(&self, rgb: [u8; 3]) -> [u8; 3] {
        [
            self.red[usize::from(rgb[0])],
            self.green[usize::from(rgb[1])],
            self.blue[usize::from(rgb[2])],
        ]
    }
}

impl DidColor {
    fn build(registers: &GbeRegisters, depth: PlaneDepth, did: u8, gamma: &GammaLut) -> Self {
        let wid = registers.wid[usize::from(did & 0x1f)];
        let mode = (wid >> 2) & 7;
        let buffer = wid & 3;
        let color_map = (wid >> 5) & 0x1f;
        let gamma_enabled = wid & (1 << 10) == 0;
        let kind = match mode {
            0 => {
                let mut table = Box::new([[0; 3]; 256]);
                for (index, entry) in table.iter_mut().enumerate() {
                    let mapped = registers.color_map[((color_map & 0xf) << 8) as usize | index];
                    let rgb = packed_rgb(mapped);
                    *entry = if gamma_enabled { gamma.apply(rgb) } else { rgb };
                }
                DidColorKind::Indexed8(table)
            }
            1 => {
                let mut table = Box::new([[0; 3]; 4_096]);
                for (index, entry) in table.iter_mut().enumerate() {
                    let rgb = packed_rgb(registers.color_map[index & 0xfff]);
                    *entry = if gamma_enabled { gamma.apply(rgb) } else { rgb };
                }
                DidColorKind::Mapped12(table)
            }
            2..=6 => DidColorKind::Direct {
                mode,
                component_map: (matches!(mode, 2..=5) && color_map != 0)
                    .then(|| Box::new(ComponentMapLut::new(registers, color_map))),
                gamma: gamma_enabled,
            },
            _ => DidColorKind::Black,
        };
        Self {
            select: BufferSelect::for_wid(depth, mode, buffer),
            kind,
        }
    }

    fn resolve(&self, raw: u32, gamma: &GammaLut) -> [u8; 3] {
        let selected = self.select.apply(raw);
        match &self.kind {
            DidColorKind::Indexed8(table) => table[(selected & 0xff) as usize],
            DidColorKind::Mapped12(table) => table[(selected & 0xfff) as usize],
            DidColorKind::Direct {
                mode,
                component_map,
                gamma: gamma_enabled,
            } => {
                let mut rgb = match mode {
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
                if let Some(map) = component_map {
                    rgb = map.apply(rgb);
                }
                if *gamma_enabled {
                    rgb = gamma.apply(rgb);
                }
                rgb
            }
            DidColorKind::Black => [0, 0, 0],
        }
    }
}

/// One display plane's framebuffer mapping captured once per composed frame.
struct PlaneSource {
    pointers: Vec<u8>,
    plane_width: usize,
    plane_height: usize,
    tiles: usize,
    depth: PlaneDepth,
}

impl PlaneSource {
    fn normal(registers: &GbeRegisters, read: &mut dyn FnMut(u64, &mut [u8]) -> bool) -> Self {
        let depth = PlaneDepth::from_frame_register(registers.frame[0]);
        let (plane_width, plane_height) = visible_dimensions(registers);
        let tiles = tile_columns(registers.frame[0]);
        let enabled = registers.frame[2] & 1 != 0;
        let base = u64::from(registers.frame[2] & !0x1f);
        Self::fetch(enabled, base, plane_width, plane_height, tiles, depth, read)
    }

    fn overlay(registers: &GbeRegisters, read: &mut dyn FnMut(u64, &mut [u8]) -> bool) -> Self {
        let plane_width = plane_width(registers.overlay[0], PlaneDepth::Eight);
        let plane_height = usize::try_from(registers.frame[1] >> 16).unwrap_or(0);
        let tiles = tile_columns(registers.overlay[0]);
        let enabled = registers.overlay[1] & 1 != 0;
        let base = u64::from(registers.overlay[1] & !0x1f);
        Self::fetch(
            enabled,
            base,
            plane_width,
            plane_height,
            tiles,
            PlaneDepth::Eight,
            read,
        )
    }

    fn fetch(
        enabled: bool,
        base: u64,
        plane_width: usize,
        plane_height: usize,
        tiles: usize,
        depth: PlaneDepth,
        read: &mut dyn FnMut(u64, &mut [u8]) -> bool,
    ) -> Self {
        let bytes = align_up(
            tiles
                .saturating_mul(plane_height.div_ceil(128))
                .saturating_mul(2),
            32,
        );
        let mut pointers = vec![0; usize::from(enabled) * bytes];
        if enabled && !read(base, &mut pointers) {
            pointers.fill(0);
        }
        Self {
            pointers,
            plane_width,
            plane_height,
            tiles: usize::from(enabled) * tiles,
            depth,
        }
    }

    /// Decodes one output line's raw pixels, zeroing hole pixels.
    fn fill_line(
        &self,
        y: usize,
        output_width: usize,
        read: &mut dyn FnMut(u64, &mut [u8]) -> bool,
        chunk: &mut [u8; 512],
        line: &mut [u32],
    ) {
        if self.plane_width == 0 || self.plane_height == 0 || self.tiles == 0 {
            line.fill(0);
            return;
        }
        let tile_width = self.depth.tile_width();
        let mut output_pixel = 0;
        while output_pixel < output_width {
            let linear_pixel = y.saturating_mul(output_width).saturating_add(output_pixel);
            let source_y = linear_pixel / self.plane_width;
            if source_y >= self.plane_height {
                break;
            }
            let source_x = linear_pixel % self.plane_width;
            let tile_x = source_x / tile_width;
            let source_pixel = source_x % tile_width;
            let pixels = (output_width - output_pixel)
                .min(self.plane_width - source_x)
                .min(tile_width - source_pixel);
            let pointer_offset = ((source_y / 128) * self.tiles + tile_x) * 2;
            let page = self
                .pointers
                .get(pointer_offset..pointer_offset + 2)
                .map(|pointer| u16::from_be_bytes([pointer[0], pointer[1]]))
                .unwrap_or(0);
            if page == 0 {
                line[output_pixel..output_pixel + pixels].fill(0);
            } else {
                let address = (u64::from(page) << 16) | (((source_y % 128) as u64) * 512);
                chunk.fill(0);
                let _ = read(address, chunk);
                unswizzle_cgi_pixel_words(chunk);
                decode_segment(
                    self.depth,
                    chunk,
                    source_pixel,
                    &mut line[output_pixel..output_pixel + pixels],
                );
            }
            output_pixel += pixels;
        }
        line[output_pixel..output_width].fill(0);
    }
}

/// Reverses the CGI word order of one 512-byte tile row in place.
///
/// Pixel tile rows are stored in memory in CGI word order: within every
/// 32-byte block, 4-byte words appear from the highest address to the lowest.
/// The display FIFO consumes them in that same order, so composition must undo
/// the permutation before interpreting bytes as big-endian pixels. Metadata
/// (tile pointers, DID tables) is stored linearly and is never swizzled.
pub(crate) fn unswizzle_cgi_pixel_words(data: &mut [u8; 512]) {
    for block in data.chunks_exact_mut(32) {
        for word in 0..4 {
            let (low, high) = (word * 4, (7 - word) * 4);
            for byte in 0..4 {
                block.swap(low + byte, high + byte);
            }
        }
    }
}

/// Decodes `pixels` big-endian pixels from one linear 512-byte tile row.
fn decode_segment(depth: PlaneDepth, data: &[u8; 512], source_pixel: usize, output: &mut [u32]) {
    debug_assert!(source_pixel.saturating_add(output.len()) <= depth.tile_width());
    let base = source_pixel * depth.bytes_per_pixel();
    let pixels = output.len();
    match depth {
        PlaneDepth::Eight => {
            for (pixel, byte) in output.iter_mut().zip(&data[base..base + pixels]) {
                *pixel = u32::from(*byte);
            }
        }
        PlaneDepth::Sixteen => {
            let bytes = &data[base..base + pixels * 2];
            for (pixel, pair) in output.iter_mut().zip(bytes.chunks_exact(2)) {
                *pixel = u32::from(u16::from_be_bytes([pair[0], pair[1]]));
            }
        }
        PlaneDepth::ThirtyTwo => {
            let bytes = &data[base..base + pixels * 4];
            for (pixel, word) in output.iter_mut().zip(bytes.chunks_exact(4)) {
                *pixel = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
            }
        }
    }
}

/// DID frame and line tables captured once per composed frame.
struct DidTables {
    frame_table: [u8; 256],
    line_blocks: std::collections::BTreeMap<u8, Box<[u8; 512]>>,
}

impl DidTables {
    fn fetch(
        registers: &GbeRegisters,
        read: &mut dyn FnMut(u64, &mut [u8]) -> bool,
    ) -> Option<Self> {
        if registers.did[0] & (1 << 16) == 0 {
            return None;
        }
        let base = u64::from(registers.did[0] & 0xffff) << 16;
        let mut frame_table = [0; 256];
        let _ = read(base, &mut frame_table);
        let mut line_blocks = std::collections::BTreeMap::new();
        for entry in frame_table.chunks_exact(4) {
            let value = u32::from_be_bytes(entry.try_into().expect("four-byte DID frame entry"));
            let block = ((value >> 6) & 0x7f) as u8;
            line_blocks.entry(block).or_insert_with(|| {
                let mut data = Box::new([0; 512]);
                let _ = read(base + u64::from(block) * 512, &mut data[..]);
                data
            });
        }
        Some(Self {
            frame_table,
            line_blocks,
        })
    }

    /// Resolves the DID of every output pixel of one line, honoring table order.
    fn fill_line_dids(&self, did_y: u16, width: usize, did_x_start: u16, output: &mut [u8]) {
        output.fill(0);
        let Some((offset, block)) = self.frame_table.chunks_exact(4).find_map(|entry| {
            let value = u32::from_be_bytes(entry.try_into().expect("four-byte DID frame entry"));
            (did_y <= (value >> 21) as u16).then_some((
                ((value >> 13) & 0xff) as usize * 2,
                ((value >> 6) & 0x7f) as u8,
            ))
        }) else {
            return;
        };
        let Some(table) = self.line_blocks.get(&block) else {
            return;
        };
        let mut covered_end: i64 = -1;
        for entry in table.get(offset..).unwrap_or_default().chunks_exact(2) {
            let value = u16::from_be_bytes([entry[0], entry[1]]);
            let x_end = i64::from(value >> 5);
            if x_end <= covered_end {
                continue;
            }
            let did = (value & 0x1f) as u8;
            for did_x in (covered_end + 1)..=x_end {
                let x = (did_x as u16).wrapping_sub(did_x_start);
                if usize::from(x) < width {
                    output[usize::from(x)] = did;
                }
            }
            covered_end = x_end;
        }
    }
}

/// Per-frame cursor shape and activation state for the composed output.
enum CursorMode {
    Disabled,
    Glyph { y: u16 },
    Crosshair,
}

/// Converts one decoded line to RGBA, applying DID, overlay, and cursor state.
#[allow(clippy::too_many_arguments)]
fn resolve_row<const DID_ENABLED: bool>(
    registers: &GbeRegisters,
    gamma: &GammaLut,
    depth: PlaneDepth,
    did_colors: &mut [Option<DidColor>; 32],
    did_line: &[u8],
    overlay_table: &[Option<[u8; 3]>; 256],
    overlay_line: &[u32],
    overlay_active: bool,
    cursor: Option<(u16, u16)>,
    normal_line: &[u32],
    row: &mut [u8],
) {
    if !DID_ENABLED {
        let color =
            did_colors[0].get_or_insert_with(|| DidColor::build(registers, depth, 0, gamma));
        resolve_row_uniform(
            color,
            registers,
            gamma,
            overlay_table,
            overlay_line,
            overlay_active,
            cursor,
            normal_line,
            row,
        );
        return;
    }
    let width = normal_line.len();
    for x in 0..width {
        let did = did_line[x];
        let color = did_colors[usize::from(did & 0x1f)]
            .get_or_insert_with(|| DidColor::build(registers, depth, did, gamma));
        let mut rgb = color.resolve(normal_line[x], gamma);
        if overlay_active && let Some(overlay) = overlay_table[(overlay_line[x] & 0xff) as usize] {
            rgb = overlay;
        }
        if let Some((cursor_x_start, cursor_y)) = cursor {
            let cursor_x = (u32::from(cursor_x_start) + x as u32) as u16 & 0x0fff;
            if let Some(cursor) =
                cursor_color(registers, usize::from(cursor_x), usize::from(cursor_y))
            {
                rgb = cursor;
            }
        }
        let offset = x * 4;
        row[offset..offset + 3].copy_from_slice(&rgb);
        row[offset + 3] = 0xff;
    }
}

/// Converts one decoded line to RGBA with a single DID for the whole frame.
#[allow(clippy::too_many_arguments)]
fn resolve_row_uniform(
    color: &DidColor,
    registers: &GbeRegisters,
    gamma: &GammaLut,
    overlay_table: &[Option<[u8; 3]>; 256],
    overlay_line: &[u32],
    overlay_active: bool,
    cursor: Option<(u16, u16)>,
    normal_line: &[u32],
    row: &mut [u8],
) {
    let width = normal_line.len();
    match &color.kind {
        DidColorKind::Indexed8(table) => {
            let select = color.select;
            for x in 0..width {
                let mut rgb = table[(select.apply(normal_line[x]) & 0xff) as usize];
                if overlay_active
                    && let Some(overlay) = overlay_table[(overlay_line[x] & 0xff) as usize]
                {
                    rgb = overlay;
                }
                if let Some((cursor_x_start, cursor_y)) = cursor {
                    let cursor_x = (u32::from(cursor_x_start) + x as u32) as u16 & 0x0fff;
                    if let Some(cursor) =
                        cursor_color(registers, usize::from(cursor_x), usize::from(cursor_y))
                    {
                        rgb = cursor;
                    }
                }
                let offset = x * 4;
                row[offset..offset + 3].copy_from_slice(&rgb);
                row[offset + 3] = 0xff;
            }
        }
        _ => {
            for x in 0..width {
                let mut rgb = color.resolve(normal_line[x], gamma);
                if overlay_active
                    && let Some(overlay) = overlay_table[(overlay_line[x] & 0xff) as usize]
                {
                    rgb = overlay;
                }
                if let Some((cursor_x_start, cursor_y)) = cursor {
                    let cursor_x = (u32::from(cursor_x_start) + x as u32) as u16 & 0x0fff;
                    if let Some(cursor) =
                        cursor_color(registers, usize::from(cursor_x), usize::from(cursor_y))
                    {
                        rgb = cursor;
                    }
                }
                let offset = x * 4;
                row[offset..offset + 3].copy_from_slice(&rgb);
                row[offset + 3] = 0xff;
            }
        }
    }
}

/// Composes one complete RGBA frame directly from memory contents.
///
/// The result matches the scanline pipeline's output for a frame whose register
/// and memory state does not change mid-frame; mid-frame changes appear here as
/// a single snapshot taken at composition time rather than as tearing.
pub(super) fn compose_frame(
    registers: &GbeRegisters,
    width: usize,
    height: usize,
    read: &mut dyn FnMut(u64, &mut [u8]) -> bool,
) -> Vec<u8> {
    let mut rgba = vec![0; width.saturating_mul(height).saturating_mul(4)];
    if width == 0 || height == 0 {
        return rgba;
    }
    let depth = PlaneDepth::from_frame_register(registers.frame[0]);
    let gamma = GammaLut::new(registers);
    let mut overlay_table = [None; 256];
    for (value, entry) in overlay_table.iter_mut().enumerate() {
        if value != 0 {
            *entry = Some(gamma.apply(packed_rgb(registers.color_map[4_352 + value])));
        }
    }
    let normal = PlaneSource::normal(registers, read);
    let overlay = PlaneSource::overlay(registers, read);
    let did_tables = DidTables::fetch(registers, read);
    let overlay_active =
        overlay.tiles != 0 && overlay.plane_width != 0 && overlay.plane_height != 0;
    let cursor_mode = match registers.cursor[1] & 3 {
        1 => CursorMode::Glyph {
            y: (registers.cursor[0] >> 16) as u16,
        },
        3 => CursorMode::Crosshair,
        _ => CursorMode::Disabled,
    };

    let total = (((registers.vt[VT_XY_MAX] >> 12) & 0x0fff) + 1) as usize;
    let span = interval_length(registers.vt[VT_VPIXEN], total);
    let pipeline_lines = span.saturating_sub(height);
    let vpixen_on = usize::from(upper_endpoint(registers.vt[VT_VPIXEN]));
    let vblank_on = upper_endpoint(registers.vt[VT_VBLANK]);
    let scan_width = u64::from((registers.vt[VT_XY_MAX] & 0x0fff) + 1);
    let cursor_start = registers.vt[CURSOR_START_XY];
    let cursor_x_start = triggered_counter(
        upper_endpoint(registers.vt[VT_HPIXEN]),
        lower_endpoint(cursor_start),
        0x0fe0,
        scan_width,
    );
    let did_x_start = lower_endpoint(registers.vt[DID_START_XY]);
    let did_y_offset = upper_endpoint(registers.vt[DID_START_XY]);

    let mut normal_line = vec![0; width];
    let mut overlay_line = vec![0; width];
    let mut did_line = Vec::new();
    if did_tables.is_some() {
        did_line = vec![0; width];
    }
    let mut chunk = [0; 512];
    let mut did_colors: [Option<DidColor>; 32] = Default::default();

    for y in 0..height {
        normal.fill_line(y, width, read, &mut chunk, &mut normal_line);
        if overlay_active {
            overlay.fill_line(y, width, read, &mut chunk, &mut overlay_line);
        }
        if let Some(tables) = &did_tables {
            tables.fill_line_dids(y as u16 + did_y_offset, width, did_x_start, &mut did_line);
        }
        let scan_y = ((vpixen_on + y + pipeline_lines) % total) as u16;
        let cursor_y = triggered_counter(
            scan_y,
            vblank_on,
            upper_endpoint(cursor_start),
            total as u64,
        );
        let cursor = match &cursor_mode {
            CursorMode::Disabled => None,
            CursorMode::Crosshair => Some((cursor_x_start, cursor_y)),
            CursorMode::Glyph { y: glyph_y, .. } => cursor_y
                .checked_sub(*glyph_y)
                .filter(|offset| *offset < 32)
                .map(|_| (cursor_x_start, cursor_y)),
        };
        let row = &mut rgba[y * width * 4..(y + 1) * width * 4];
        if did_tables.is_some() {
            resolve_row::<true>(
                registers,
                &gamma,
                depth,
                &mut did_colors,
                &did_line,
                &overlay_table,
                &overlay_line,
                overlay_active,
                cursor,
                &normal_line,
                row,
            );
        } else {
            resolve_row::<false>(
                registers,
                &gamma,
                depth,
                &mut did_colors,
                &did_line,
                &overlay_table,
                &overlay_line,
                overlay_active,
                cursor,
                &normal_line,
                row,
            );
        }
    }
    rgba
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

        registers.cursor[0] = 0;
        registers.cursor[1] = 1;
        registers.cursor[2] = 0x0a14_1e00;
        registers.cursor_glyph[0] = 1 << 30;
        assert_eq!(cursor_color(&registers, 0, 0), Some([10, 20, 30]));
    }

    #[test]
    fn cursor_position_is_the_upper_left_glyph_origin() {
        let mut registers = GbeRegisters::new();
        registers.cursor[0] = (7 << 16) | 5;
        registers.cursor[1] = 1;
        registers.cursor[2] = 0x4455_6600;
        registers.cursor_glyph[0] = 1 << 30;
        assert_eq!(cursor_color(&registers, 5, 7), Some([0x44, 0x55, 0x66]));
        assert_eq!(cursor_color(&registers, 4, 7), None);
        assert_eq!(cursor_color(&registers, 5, 6), None);
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
