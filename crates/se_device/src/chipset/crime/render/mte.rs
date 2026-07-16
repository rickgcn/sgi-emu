//! Memory Transfer Engine command snapshots and execution state.

use super::{
    FRAMEBUFFER_HEIGHT, FRAMEBUFFER_TILE_HEIGHT, FRAMEBUFFER_TILE_ROW_BYTES,
    FRAMEBUFFER_TILES_PER_ROW, FramebufferTlbEntry, LINEAR_PAGE_COUNT, LINEAR_PAGE_SIZE,
    LinearTlbEntry, MAX_MEMORY_CHUNK_BYTES, MteInvalidField, MteRegisters, RenderTlbs,
    SemanticFallbackProvenance, framebuffer,
};

const FRAMEBUFFER_TLB_ENTRY_COUNT: usize = 256;
const TEXTURE_TLB_ENTRY_COUNT: usize = 112;
const CID_TLB_ENTRY_COUNT: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(super) enum MteOperation {
    Clear,
    Copy,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(super) enum MteBufferSnapshot {
    Tiled {
        entries: Vec<u16>,
        maximum_height: u16,
    },
    Linear {
        entries: [u32; LINEAR_PAGE_COUNT],
    },
}

impl MteBufferSnapshot {
    pub(super) fn capture(tlbs: &RenderTlbs, selector: u8) -> Option<Self> {
        match selector {
            0 => Some(Self::tiled(
                &tlbs.framebuffer_a,
                FRAMEBUFFER_TLB_ENTRY_COUNT,
            )),
            1 => Some(Self::tiled(
                &tlbs.framebuffer_b,
                FRAMEBUFFER_TLB_ENTRY_COUNT,
            )),
            2 => Some(Self::tiled(
                &tlbs.framebuffer_c,
                FRAMEBUFFER_TLB_ENTRY_COUNT,
            )),
            3 => Some(Self::tiled(&tlbs.texture, TEXTURE_TLB_ENTRY_COUNT)),
            4 => Some(Self::Linear {
                entries: tlbs.linear_a_entries(),
            }),
            5 => Some(Self::Linear {
                entries: tlbs.linear_b_entries(),
            }),
            6 => Some(Self::tiled(&tlbs.cid, CID_TLB_ENTRY_COUNT)),
            _ => None,
        }
    }

    fn tiled(slots: &[u64], entry_count: usize) -> Self {
        let mut entries = Vec::with_capacity(entry_count);
        for slot in slots {
            entries.extend([
                (slot >> 48) as u16,
                (slot >> 32) as u16,
                (slot >> 16) as u16,
                *slot as u16,
            ]);
        }
        entries.truncate(entry_count);
        let tile_rows = entry_count.div_ceil(FRAMEBUFFER_TILES_PER_ROW);
        Self::Tiled {
            entries,
            maximum_height: (tile_rows * usize::from(FRAMEBUFFER_TILE_HEIGHT)) as u16,
        }
    }

    pub(super) const fn linear(&self) -> bool {
        matches!(self, Self::Linear { .. })
    }

    pub(super) fn translate_address(
        &self,
        address: u32,
        bytes_per_pixel: u8,
        physical_pixel_layout: bool,
    ) -> Option<MteTranslation> {
        let end = if self.linear() {
            address.checked_add(u32::from(bytes_per_pixel) - 1)?
        } else {
            let x = address as u16;
            let y_byte = address >> 16;
            y_byte
                .checked_add(u32::from(bytes_per_pixel) - 1)?
                .checked_shl(16)?
                | u32::from(x)
        };
        let cursor = MteCursor::new(self, address, end, bytes_per_pixel, 0, false).ok()?;
        Some(cursor.translate(self, bytes_per_pixel, physical_pixel_layout))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(super) struct MteTranslation {
    pub(super) virtual_address: u32,
    pub(super) raw_entry: u32,
    pub(super) valid: bool,
    pub(super) alias_address: u64,
    pub(super) contiguous_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(super) enum MteCursor {
    Linear {
        start: u64,
        end: u64,
        current: u64,
        reverse: bool,
        done: bool,
    },
    Tiled {
        x_start: u16,
        x_end: u16,
        y_start: u16,
        y_end: u16,
        x: u16,
        y: u16,
        row_step: u16,
        reverse: bool,
        done: bool,
    },
}

impl MteCursor {
    pub(super) fn new(
        buffer: &MteBufferSnapshot,
        start: u32,
        end: u32,
        bytes_per_pixel: u8,
        y_step: u32,
        reverse: bool,
    ) -> Result<Self, MteInvalidField> {
        if buffer.linear() {
            if end < start
                || (u64::from(end) - u64::from(start) + 1) % u64::from(bytes_per_pixel) != 0
            {
                return Err(MteInvalidField::Range);
            }
            let current = if reverse {
                u64::from(end) + 1 - u64::from(bytes_per_pixel)
            } else {
                u64::from(start)
            };
            return Ok(Self::Linear {
                start: u64::from(start),
                end: u64::from(end),
                current,
                reverse,
                done: false,
            });
        }

        let MteBufferSnapshot::Tiled { maximum_height, .. } = buffer else {
            unreachable!("linear buffer returned above")
        };
        let bytes = u16::from(bytes_per_pixel);
        let x_start = start as u16;
        let x_end = end as u16;
        let y_byte_start = (start >> 16) as u16;
        let y_byte_end = (end >> 16) as u16;
        let y_start = y_byte_start / bytes;
        let y_end = y_byte_end / bytes;
        let tile_width = (FRAMEBUFFER_TILE_ROW_BYTES / u64::from(bytes_per_pixel)) as u16;
        let maximum_width = tile_width * FRAMEBUFFER_TILES_PER_ROW as u16;
        if x_end < x_start
            || !y_byte_start.is_multiple_of(bytes)
            || y_byte_end % bytes != bytes - 1
            || y_end < y_start
            || x_end >= maximum_width
            || y_end >= *maximum_height
            || y_end >= FRAMEBUFFER_HEIGHT
        {
            return Err(MteInvalidField::Range);
        }
        let row_step = if y_step == 0 {
            1
        } else {
            u16::try_from(y_step).map_err(|_| MteInvalidField::YStep)?
        };
        Ok(Self::Tiled {
            x_start,
            x_end,
            y_start,
            y_end,
            x: if reverse { x_end } else { x_start },
            y: if reverse { y_end } else { y_start },
            row_step,
            reverse,
            done: false,
        })
    }

    pub(super) const fn complete(&self) -> bool {
        match self {
            Self::Linear { done, .. } | Self::Tiled { done, .. } => *done,
        }
    }

    pub(super) fn virtual_address(&self, bytes_per_pixel: u8) -> u32 {
        match self {
            Self::Linear { current, .. } => *current as u32,
            Self::Tiled { x, y, .. } => (u32::from(*y) * bytes_per_pixel as u32) << 16 | *x as u32,
        }
    }

    pub(super) fn pixel_count(&self, bytes_per_pixel: u8) -> u64 {
        match self {
            Self::Linear { start, end, .. } => (end - start + 1) / u64::from(bytes_per_pixel),
            Self::Tiled {
                x_start,
                x_end,
                y_start,
                y_end,
                row_step,
                ..
            } => {
                let width = u64::from(*x_end) - u64::from(*x_start) + 1;
                let rows = (u64::from(*y_end) - u64::from(*y_start)) / u64::from(*row_step) + 1;
                width * rows
            }
        }
    }

    pub(super) fn remaining_pixels(&self, bytes_per_pixel: u8) -> u64 {
        match self {
            Self::Linear {
                start,
                end,
                current,
                reverse,
                done,
            } => {
                if *done {
                    0
                } else if *reverse {
                    (current - start) / u64::from(bytes_per_pixel) + 1
                } else {
                    (end + 1 - current) / u64::from(bytes_per_pixel)
                }
            }
            Self::Tiled {
                x_start,
                x_end,
                y_start,
                y_end,
                x,
                y,
                row_step,
                reverse,
                done,
            } => {
                if *done {
                    return 0;
                }
                let width = u64::from(*x_end) - u64::from(*x_start) + 1;
                if *reverse {
                    let current_row = u64::from(*x) - u64::from(*x_start) + 1;
                    let preceding_rows =
                        (u64::from(*y) - u64::from(*y_start)) / u64::from(*row_step);
                    current_row + preceding_rows * width
                } else {
                    let current_row = u64::from(*x_end) - u64::from(*x) + 1;
                    let following_rows = (u64::from(*y_end) - u64::from(*y)) / u64::from(*row_step);
                    current_row + following_rows * width
                }
            }
        }
    }

    pub(super) fn translate(
        &self,
        buffer: &MteBufferSnapshot,
        bytes_per_pixel: u8,
        physical_pixel_layout: bool,
    ) -> MteTranslation {
        match (self, buffer) {
            (Self::Linear { current, .. }, MteBufferSnapshot::Linear { entries }) => {
                let page_index = ((*current >> 12) & 0x1f) as usize;
                let in_page = *current & (LINEAR_PAGE_SIZE - 1);
                let entry = LinearTlbEntry(entries[page_index]);
                MteTranslation {
                    virtual_address: *current as u32,
                    raw_entry: entry.0,
                    valid: entry.valid(),
                    alias_address: entry.alias_address(in_page),
                    contiguous_bytes: (LINEAR_PAGE_SIZE - in_page) as usize,
                }
            }
            (Self::Tiled { x, y, .. }, MteBufferSnapshot::Tiled { entries, .. }) => {
                let tile_width = (FRAMEBUFFER_TILE_ROW_BYTES / u64::from(bytes_per_pixel)) as u16;
                let tile_x = usize::from(*x / tile_width);
                let tile_y = usize::from(*y / FRAMEBUFFER_TILE_HEIGHT);
                let entry =
                    FramebufferTlbEntry(entries[tile_y * FRAMEBUFFER_TILES_PER_ROW + tile_x]);
                let x_in_tile = *x % tile_width;
                let y_in_tile = *y % FRAMEBUFFER_TILE_HEIGHT;
                let tile_offset = u64::from(y_in_tile) * FRAMEBUFFER_TILE_ROW_BYTES
                    + u64::from(x_in_tile) * u64::from(bytes_per_pixel);
                let logical_alias = entry.alias_address(tile_offset);
                let alias_address = if physical_pixel_layout {
                    let word = logical_alias & !31;
                    let logical_lane = (logical_alias - word) as usize;
                    let physical_lane = framebuffer::physical_pixel_lane(
                        logical_lane,
                        usize::from(bytes_per_pixel),
                    )
                    .expect("validated MTE pixel remains in one framebuffer word");
                    word + physical_lane as u64
                } else {
                    logical_alias
                };
                MteTranslation {
                    virtual_address: (u32::from(*y) * u32::from(bytes_per_pixel)) << 16
                        | u32::from(*x),
                    raw_entry: u32::from(entry.raw()),
                    valid: entry.valid(),
                    alias_address,
                    contiguous_bytes: usize::from(tile_width - x_in_tile)
                        * usize::from(bytes_per_pixel),
                }
            }
            _ => unreachable!("cursor and buffer kinds are captured together"),
        }
    }

    pub(super) fn advance(&mut self, byte_count: usize, bytes_per_pixel: u8) -> bool {
        let pixels = byte_count / usize::from(bytes_per_pixel);
        let mut crossed_row = false;
        for _ in 0..pixels {
            match self {
                Self::Linear {
                    start,
                    end,
                    current,
                    reverse,
                    done,
                } => {
                    if *reverse {
                        if *current == *start {
                            *done = true;
                        } else {
                            *current -= u64::from(bytes_per_pixel);
                        }
                    } else if *current + u64::from(bytes_per_pixel) > *end {
                        *done = true;
                    } else {
                        *current += u64::from(bytes_per_pixel);
                    }
                    crossed_row = *done;
                }
                Self::Tiled {
                    x_start,
                    x_end,
                    y_start,
                    y_end,
                    x,
                    y,
                    row_step,
                    reverse,
                    done,
                } => {
                    if *reverse {
                        if *x > *x_start {
                            *x -= 1;
                        } else if *y < *y_start + *row_step {
                            *done = true;
                            crossed_row = true;
                        } else {
                            *x = *x_end;
                            *y -= *row_step;
                            crossed_row = true;
                        }
                    } else if *x < *x_end {
                        *x += 1;
                    } else if u32::from(*y) + u32::from(*row_step) > u32::from(*y_end) {
                        *done = true;
                        crossed_row = true;
                    } else {
                        *x = *x_start;
                        *y += *row_step;
                        crossed_row = true;
                    }
                }
            }
        }
        crossed_row
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(super) enum MteStage {
    Clear,
    CopyRead,
    CopyWrite,
    Complete,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(super) struct MteJob {
    pub(super) start: u32,
    pub(super) end: u32,
    pub(super) operation: MteOperation,
    pub(super) bytes_per_pixel: u8,
    pub(super) byte_mask: u32,
    pub(super) stipple_mask: u32,
    pub(super) stipple_enabled: bool,
    pub(super) foreground: u32,
    pub(super) source_ecc: bool,
    pub(super) destination_ecc: bool,
    pub(super) source_buffer: Option<MteBufferSnapshot>,
    pub(super) destination_buffer: MteBufferSnapshot,
    pub(super) source: Option<MteCursor>,
    pub(super) destination: MteCursor,
    pub(super) stage: MteStage,
    pub(super) row_buffer: Vec<u8>,
    pub(super) row_write_offset: usize,
    pub(super) processed_pixels: u64,
    pub(super) semantic_fallbacks: SemanticFallbackProvenance,
}

impl MteJob {
    pub(super) fn snapshot(
        registers: MteRegisters,
        tlbs: &RenderTlbs,
    ) -> Result<Self, MteInvalidField> {
        let mode = registers.mode;
        if mode & !0x0fff != 0 {
            return Err(MteInvalidField::ReservedModeBits);
        }
        let operation = if mode & (1 << 11) == 0 {
            MteOperation::Clear
        } else {
            MteOperation::Copy
        };
        let depth = ((mode >> 8) & 3) as u8;
        if depth == 3 {
            return Err(MteInvalidField::PixelDepth);
        }
        let bytes_per_pixel = 1_u8 << depth;
        let source_selector = ((mode >> 5) & 7) as u8;
        let destination_selector = ((mode >> 2) & 7) as u8;
        let destination_buffer = MteBufferSnapshot::capture(tlbs, destination_selector)
            .ok_or(MteInvalidField::DestinationBuffer)?;
        let source_buffer = if operation == MteOperation::Copy {
            Some(
                MteBufferSnapshot::capture(tlbs, source_selector)
                    .ok_or(MteInvalidField::SourceBuffer)?,
            )
        } else {
            None
        };
        let reverse = operation == MteOperation::Copy
            && source_selector == destination_selector
            && registers.destination_start > registers.source_start
            && registers.destination_start <= registers.source_end;
        let destination = MteCursor::new(
            &destination_buffer,
            registers.destination_start,
            registers.destination_end,
            bytes_per_pixel,
            registers.destination_y_step,
            reverse,
        )?;
        let source = source_buffer
            .as_ref()
            .map(|buffer| {
                MteCursor::new(
                    buffer,
                    registers.source_start,
                    registers.source_end,
                    bytes_per_pixel,
                    registers.source_y_step,
                    reverse,
                )
            })
            .transpose()?;
        if let Some(source) = source.as_ref()
            && source.pixel_count(bytes_per_pixel) != destination.pixel_count(bytes_per_pixel)
        {
            return Err(MteInvalidField::CopyShape);
        }
        Ok(Self {
            start: registers.destination_start,
            end: registers.destination_end,
            operation,
            bytes_per_pixel,
            byte_mask: registers.byte_mask,
            stipple_mask: registers.stipple_mask,
            stipple_enabled: mode & (1 << 10) != 0,
            foreground: registers.foreground,
            source_ecc: mode & 2 != 0,
            destination_ecc: mode & 1 != 0,
            source_buffer,
            destination_buffer,
            source,
            destination,
            stage: match operation {
                MteOperation::Clear => MteStage::Clear,
                MteOperation::Copy => MteStage::CopyRead,
            },
            row_buffer: Vec::new(),
            row_write_offset: 0,
            processed_pixels: 0,
            semantic_fallbacks: if reverse {
                SemanticFallbackProvenance::mte_overlap()
            } else {
                SemanticFallbackProvenance::default()
            },
        })
    }

    pub(super) const fn complete(&self) -> bool {
        matches!(self.stage, MteStage::Complete)
    }

    pub(super) fn clear_transfer(&self) -> (MteTranslation, Vec<u8>, Vec<bool>) {
        let general = self.foreground != 0 || self.byte_mask != u32::MAX || self.stipple_enabled;
        let translation = self.destination.translate(
            &self.destination_buffer,
            self.bytes_per_pixel,
            general && !self.destination_buffer.linear(),
        );
        let remaining_bytes = self.destination.remaining_pixels(self.bytes_per_pixel)
            * u64::from(self.bytes_per_pixel);
        let length = if general {
            usize::from(self.bytes_per_pixel)
        } else {
            usize::try_from(remaining_bytes)
                .unwrap_or(usize::MAX)
                .min(translation.contiguous_bytes)
                .min(MAX_MEMORY_CHUNK_BYTES)
        };
        let foreground = self.foreground.to_be_bytes();
        let component_start = 4 - usize::from(self.bytes_per_pixel);
        let mut data = Vec::with_capacity(length);
        let mut byte_enable = Vec::with_capacity(length);
        let virtual_address = self.destination.virtual_address(self.bytes_per_pixel);
        for index in 0..length {
            let pixel = self.processed_pixels + (index / usize::from(self.bytes_per_pixel)) as u64;
            let pixel_enabled =
                !self.stipple_enabled || self.stipple_mask & (1 << (31 - (pixel & 31))) != 0;
            let byte_index = index % usize::from(self.bytes_per_pixel);
            data.push(foreground[component_start + byte_index]);
            let mask_index = (u64::from(virtual_address) + index as u64) & 31;
            byte_enable.push(pixel_enabled && self.byte_mask & (1 << (31 - mask_index)) != 0);
        }
        (translation, data, byte_enable)
    }

    pub(super) fn copy_read(&self) -> MteTranslation {
        let source = self.source.as_ref().expect("copy job has a source cursor");
        source.translate(
            self.source_buffer
                .as_ref()
                .expect("copy job has a source buffer"),
            self.bytes_per_pixel,
            true,
        )
    }

    pub(super) fn copy_write(&self) -> (MteTranslation, &[u8]) {
        let translation =
            self.destination
                .translate(&self.destination_buffer, self.bytes_per_pixel, true);
        let end = self.row_write_offset + usize::from(self.bytes_per_pixel);
        (translation, &self.row_buffer[self.row_write_offset..end])
    }

    pub(super) fn finish_clear(&mut self, length: usize) {
        self.destination.advance(length, self.bytes_per_pixel);
        self.processed_pixels += (length / usize::from(self.bytes_per_pixel)) as u64;
        if self.destination.complete() {
            self.stage = MteStage::Complete;
        }
    }

    pub(super) fn finish_copy_read(&mut self, bytes: &[u8]) {
        self.row_buffer.extend_from_slice(bytes);
        let source = self.source.as_mut().expect("copy job has a source cursor");
        let crossed_row = source.advance(bytes.len(), self.bytes_per_pixel);
        if crossed_row || source.complete() {
            self.row_write_offset = 0;
            self.stage = MteStage::CopyWrite;
        }
    }

    pub(super) fn finish_copy_write(&mut self, length: usize) {
        let crossed_row = self.destination.advance(length, self.bytes_per_pixel);
        self.row_write_offset += length;
        self.processed_pixels += (length / usize::from(self.bytes_per_pixel)) as u64;
        if crossed_row || self.destination.complete() {
            self.row_buffer.clear();
            self.row_write_offset = 0;
            let source_complete = self.source.as_ref().is_none_or(MteCursor::complete);
            if source_complete && self.destination.complete() {
                self.stage = MteStage::Complete;
            } else {
                self.stage = MteStage::CopyRead;
            }
        }
    }
}
