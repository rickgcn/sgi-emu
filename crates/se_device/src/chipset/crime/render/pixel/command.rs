//! PixelPipe command snapshots, typed decoding, and aggregate validation.

#[cfg(test)]
use super::super::PIXEL_PIPE_NULL;
use super::super::{
    PIXEL_PIPE_BASE, PixelCommandViolation, PixelField, PixelRegister, PixelViolationKind,
    read_register_slot, write_register_slot,
};
use super::format::{Rgba8, encode};
use super::stipple::PixelStippleMode;

const DRAW_MODE_DEFINED_MASK: u32 = 0x00ff_ffff;
const PRIMITIVE_RESERVED_MASK: u32 = 0x00f8_0000;
const BUFFER_MODE_RESERVED_MASK: u32 = 0xffff_e000;
const CLIP_MODE_RESERVED_MASK: u32 = 0xffff_f000;

const FEATURE_GL: u8 = 22;
const FEATURE_PIXEL_TRANSFER: u8 = 21;
const FEATURE_SCISSOR: u8 = 20;
const FEATURE_LINE_STIPPLE: u8 = 19;
const FEATURE_POLYGON_STIPPLE: u8 = 18;
const FEATURE_OPAQUE_STIPPLE: u8 = 17;
const FEATURE_SHADE: u8 = 16;
const FEATURE_TEXTURE: u8 = 15;
const FEATURE_FOG: u8 = 14;
const FEATURE_COVERAGE: u8 = 13;
const FEATURE_ANTIALIAS_LINE: u8 = 12;
const FEATURE_ALPHA_TEST: u8 = 11;
const FEATURE_BLEND: u8 = 10;
const FEATURE_DITHER: u8 = 9;
const FEATURE_LOGIC_OPERATION: u8 = 8;
const FEATURE_COLOR_MASK: u8 = 7;
const FEATURE_DEPTH_TEST: u8 = 2;
const FEATURE_DEPTH_MASK: u8 = 1;
const FEATURE_STENCIL_TEST: u8 = 0;

/// Raw PixelPipe register storage.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct PixelRegisters {
    #[serde(with = "crate::common::serde_array")]
    pub(crate) slots: [u64; 64],
}

impl PixelRegisters {
    pub(crate) const fn new() -> Self {
        Self { slots: [0; 64] }
    }

    pub(crate) fn reset(&mut self) {
        self.slots.fill(0);
    }

    pub(crate) fn read(&self, address: u64, size: u8) -> u64 {
        read_register_slot(&self.slots, address - PIXEL_PIPE_BASE, size)
    }

    pub(crate) fn write(&mut self, address: u64, size: u8, value: u64) {
        write_register_slot(&mut self.slots, address - PIXEL_PIPE_BASE, size, value);
    }

    pub(crate) fn command_snapshot(&self, trigger_address: u64) -> PixelCommandSnapshot {
        PixelCommandSnapshot {
            trigger_address,
            registers: self.clone(),
        }
    }
}

/// Immutable PixelPipe register state captured by a START write.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct PixelCommandSnapshot {
    pub(crate) trigger_address: u64,
    registers: PixelRegisters,
}

impl PixelCommandSnapshot {
    pub(crate) fn register32(&self, offset: u64) -> u32 {
        self.registers.read(PIXEL_PIPE_BASE + offset, 4) as u32
    }

    pub(crate) fn register64(&self, offset: u64) -> u64 {
        self.registers.read(PIXEL_PIPE_BASE + offset, 8)
    }

    pub(crate) fn primitive(&self) -> u32 {
        self.register32(0x060)
    }

    pub(crate) fn draw_mode(&self) -> u32 {
        self.register32(0x018)
    }

    pub(crate) fn source_buffer_mode(&self) -> u32 {
        self.register32(0x000)
    }

    pub(crate) fn destination_buffer_mode(&self) -> u32 {
        self.register32(0x008)
    }

    pub(crate) fn clip_mode(&self) -> u32 {
        self.register32(0x010)
    }

    pub(crate) fn destination_window_offset(&self) -> u32 {
        self.register32(0x058)
    }

    pub(crate) fn source_window_offset(&self) -> u32 {
        self.register32(0x050)
    }

    pub(crate) fn pixel_transfer_source_address(&self) -> u32 {
        self.register32(0x0a0)
    }

    pub(crate) fn pixel_transfer_source_x_step(&self) -> i32 {
        self.register32(0x0a8) as i32
    }

    pub(crate) fn pixel_transfer_source_y_step(&self) -> i32 {
        self.register32(0x0ac) as i32
    }

    pub(crate) fn pixel_transfer_destination_address(&self) -> u32 {
        self.register32(0x0b0)
    }

    pub(crate) fn pixel_transfer_destination_stride(&self) -> i32 {
        self.register32(0x0b4) as i32
    }

    pub(crate) fn foreground_color(&self) -> u32 {
        self.register32(0x0d0)
    }

    pub(crate) fn background_color(&self) -> u32 {
        self.register32(0x0d8)
    }

    pub(crate) fn logic_operation(&self) -> u32 {
        self.register32(0x1b0)
    }

    pub(crate) fn color_mask(&self) -> u32 {
        self.register32(0x1b8)
    }

    pub(crate) fn x_vertex(&self, index: usize) -> (u16, u16) {
        let value = self.register32(0x070 + index as u64 * 4);
        ((value >> 16) as u16, value as u16)
    }

    pub(crate) fn gl_vertex(&self, index: usize) -> (i32, i32) {
        let x = self.register32(0x080 + index as u64 * 8) as i32;
        let y = self.register32(0x084 + index as u64 * 8) as i32;
        (x, y)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct PixelRectangle {
    pub(crate) min_x: u16,
    pub(crate) min_y: u16,
    pub(crate) max_x: u16,
    pub(crate) max_y: u16,
}

impl PixelRectangle {
    const fn decode(raw: u64) -> Self {
        Self {
            min_x: (raw >> 48) as u16,
            min_y: (raw >> 32) as u16,
            max_x: (raw >> 16) as u16,
            max_y: raw as u16,
        }
    }

    const fn contains(self, x: u16, y: u16) -> bool {
        x >= self.min_x && x < self.max_x && y >= self.min_y && y < self.max_y
    }
}

/// Primitive kind decoded independently from its remaining fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) enum PixelPrimitiveKind {
    Point,
    Line,
    Triangle,
    Rectangle,
    Flush,
    Invalid(u8),
}

/// Buffer selected by one BufMode register.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) enum PixelBufferKind {
    FramebufferA,
    FramebufferB,
    FramebufferC,
    LinearA,
    LinearB,
    Invalid(u8),
}

impl PixelBufferKind {
    pub(crate) const fn framebuffer_selector(self) -> Option<u8> {
        match self {
            Self::FramebufferA => Some(0),
            Self::FramebufferB => Some(1),
            Self::FramebufferC => Some(2),
            _ => None,
        }
    }

    pub(crate) const fn buffer_selector(self) -> Option<u8> {
        match self {
            Self::FramebufferA => Some(0),
            Self::FramebufferB => Some(1),
            Self::FramebufferC => Some(2),
            Self::LinearA => Some(4),
            Self::LinearB => Some(5),
            Self::Invalid(_) => None,
        }
    }
}

/// Pixel type and depth decoded from a BufMode register.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) enum PixelFormat {
    ColorIndex(u8),
    Rgb(u8),
    Rgba(u8),
    Abgr(u8),
    YCrCb(u8),
    Invalid { pixel_type: u8, pixel_depth: u8 },
}

impl PixelFormat {
    pub(crate) const fn bytes_per_pixel(self) -> Option<u8> {
        match self {
            Self::ColorIndex(8) | Self::Rgb(8) | Self::Rgba(8) | Self::Abgr(8) => Some(1),
            Self::ColorIndex(16) | Self::Rgb(16) | Self::Rgba(16) | Self::Abgr(16) => Some(2),
            Self::Rgb(32) | Self::Rgba(32) | Self::Abgr(32) | Self::YCrCb(32) => Some(4),
            _ => None,
        }
    }
}

/// Typed BufMode register.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct PixelBufferMode {
    pub(crate) raw: u32,
    pub(crate) kind: PixelBufferKind,
    pub(crate) buffer_depth: u8,
    pub(crate) format: PixelFormat,
    pub(crate) pixel_depth: u8,
    pub(crate) double_pixel: bool,
    pub(crate) double_pixel_select: bool,
}

/// All 24 defined DrawMode bits using the target IRIX bit assignment.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct PixelFeatureSet(u32);

impl PixelFeatureSet {
    pub(crate) const fn new(draw_mode: u32) -> Self {
        Self(draw_mode & DRAW_MODE_DEFINED_MASK)
    }

    pub(crate) const fn raw(self) -> u32 {
        self.0
    }

    pub(crate) const fn enabled(self, bit: u8) -> bool {
        self.0 & (1_u32 << bit) != 0
    }

    pub(crate) const fn color_byte_mask(self) -> u8 {
        ((self.0 >> 3) & 0x0f) as u8
    }

    pub(crate) const fn pixel_transfer(self) -> bool {
        self.enabled(FEATURE_PIXEL_TRANSFER)
    }

    pub(crate) const fn gl(self) -> bool {
        self.enabled(FEATURE_GL)
    }

    pub(crate) const fn polygon_stipple(self) -> bool {
        self.enabled(FEATURE_POLYGON_STIPPLE)
    }

    pub(crate) const fn opaque_stipple(self) -> bool {
        self.enabled(FEATURE_OPAQUE_STIPPLE)
    }

    pub(crate) const fn smooth_shade(self) -> bool {
        self.enabled(FEATURE_SHADE)
    }

    pub(crate) const fn texture(self) -> bool {
        self.enabled(FEATURE_TEXTURE)
    }

    pub(crate) const fn fog(self) -> bool {
        self.enabled(FEATURE_FOG)
    }

    pub(crate) const fn coverage(self) -> bool {
        self.enabled(FEATURE_COVERAGE) || self.enabled(FEATURE_ANTIALIAS_LINE)
    }

    pub(crate) const fn line_antialias(self) -> bool {
        self.enabled(FEATURE_ANTIALIAS_LINE)
    }

    pub(crate) const fn alpha_test(self) -> bool {
        self.enabled(FEATURE_ALPHA_TEST)
    }

    pub(crate) const fn blend(self) -> bool {
        self.enabled(FEATURE_BLEND)
    }

    pub(crate) const fn logic(self) -> bool {
        self.enabled(FEATURE_LOGIC_OPERATION)
    }

    pub(crate) const fn dither(self) -> bool {
        self.enabled(FEATURE_DITHER)
    }

    pub(crate) const fn color_mask(self) -> bool {
        self.enabled(FEATURE_COLOR_MASK)
    }

    pub(crate) const fn depth_test(self) -> bool {
        self.enabled(FEATURE_DEPTH_TEST)
    }

    pub(crate) const fn depth_mask(self) -> bool {
        self.enabled(FEATURE_DEPTH_MASK)
    }

    pub(crate) const fn stencil_test(self) -> bool {
        self.enabled(FEATURE_STENCIL_TEST)
    }
}

/// Fully decoded immutable PixelPipe command.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct DecodedPixelCommand {
    pub(crate) snapshot: PixelCommandSnapshot,
    pub(crate) primitive_kind: PixelPrimitiveKind,
    pub(crate) primitive_raw: u32,
    pub(crate) line_width: u16,
    pub(crate) edge_type: u8,
    pub(crate) skip_last_endpoint: bool,
    pub(crate) features: PixelFeatureSet,
    pub(crate) source: PixelBufferMode,
    pub(crate) destination: PixelBufferMode,
    pub(crate) stipple_mode: PixelStippleMode,
    pub(crate) stipple_pattern: u32,
    pub(crate) foreground_color: u32,
    pub(crate) background_color: u32,
    pub(crate) clip_mode: u32,
    pub(crate) screen_masks: [PixelRectangle; 5],
    pub(crate) scissor: PixelRectangle,
    pub(crate) source_window_offset: (u16, u16),
    pub(crate) destination_window_offset: (u16, u16),
    pub(crate) x0: u16,
    pub(crate) y0: u16,
    pub(crate) x1: u16,
    pub(crate) y1: u16,
    pub(crate) x2: u16,
    pub(crate) y2: u16,
    pub(crate) vertices_subpixel: [(i32, i32); 3],
}

impl DecodedPixelCommand {
    pub(crate) fn endpoints(&self) -> (u16, u16, u16, u16) {
        (self.x0, self.y0, self.x1, self.y1)
    }

    pub(crate) fn line_stipple_enabled(&self) -> bool {
        self.features.enabled(FEATURE_LINE_STIPPLE)
    }

    pub(crate) fn pixel_bytes(&self) -> Vec<u8> {
        self.color_bytes(self.foreground_color)
    }

    pub(crate) fn background_pixel_bytes(&self) -> Vec<u8> {
        self.color_bytes(self.background_color)
    }

    fn color_bytes(&self, color: u32) -> Vec<u8> {
        match self.destination.format {
            PixelFormat::ColorIndex(8) => vec![color as u8],
            PixelFormat::ColorIndex(16) => (color as u16 & 0x0fff).to_be_bytes().to_vec(),
            PixelFormat::ColorIndex(32) => (color & 0x0fff).to_be_bytes().to_vec(),
            format => {
                let [r, g, b, a] = color.to_be_bytes();
                encode(format, Rgba8 { r, g, b, a })
            }
        }
    }

    pub(crate) fn framebuffer_position(&self, x: u16, y: u16) -> Option<(u16, u16)> {
        Some((
            x.checked_add(self.destination_window_offset.0)?,
            y.checked_add(self.destination_window_offset.1)?,
        ))
    }

    pub(crate) fn source_framebuffer_position(&self, x: u16, y: u16) -> Option<(u16, u16)> {
        Some((
            x.checked_add(self.source_window_offset.0)?,
            y.checked_add(self.source_window_offset.1)?,
        ))
    }

    pub(crate) fn clip_passes(&self, window_x: u16, window_y: u16) -> bool {
        if self.features.enabled(FEATURE_SCISSOR) && !self.scissor.contains(window_x, window_y) {
            return false;
        }
        let Some((framebuffer_x, framebuffer_y)) = self.framebuffer_position(window_x, window_y)
        else {
            return false;
        };
        for index in 0..5 {
            if self.clip_mode & (1 << (9 - index)) == 0 {
                continue;
            }
            let inside = self.screen_masks[index].contains(framebuffer_x, framebuffer_y);
            let require_inside = self.clip_mode & (1 << (4 - index)) != 0;
            if inside != require_inside {
                return false;
            }
        }
        true
    }

    pub(crate) fn needs_fragment_pipeline(&self) -> bool {
        self.features.smooth_shade()
            || self.features.texture()
            || self.features.fog()
            || self.features.coverage()
            || self.features.alpha_test()
            || self.features.blend()
            || self.features.dither()
                && matches!(
                    self.destination.format,
                    PixelFormat::Rgb(8 | 16)
                        | PixelFormat::Rgba(8 | 16)
                        | PixelFormat::Abgr(8 | 16)
                )
            || self.features.logic() && self.snapshot.logic_operation() & 0x0f != 3
            || self.features.color_mask() && self.snapshot.color_mask() != u32::MAX
            || self.features.color_byte_mask() != 0x0f
            || self.features.depth_test()
            || self.features.depth_mask()
            || self.features.stencil_test()
            || self.destination.double_pixel
            || self.destination.kind.framebuffer_selector().is_none()
            || self.clip_mode & (1 << 11) != 0
    }
}

/// Aggregate result of decoding and validating one command snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PixelCommandValidation {
    pub(crate) decoded: DecodedPixelCommand,
    pub(crate) violations: Vec<PixelCommandViolation>,
}

impl PixelCommandValidation {
    pub(crate) fn decode(snapshot: PixelCommandSnapshot) -> Self {
        let primitive_raw = snapshot.primitive();
        let draw_mode = snapshot.draw_mode();
        let source_raw = snapshot.source_buffer_mode();
        let destination_raw = snapshot.destination_buffer_mode();
        let source = decode_buffer_mode(source_raw);
        let destination = decode_buffer_mode(destination_raw);
        let opcode = (primitive_raw >> 24) as u8;
        let primitive_kind = match opcode {
            0 => PixelPrimitiveKind::Point,
            1 => PixelPrimitiveKind::Line,
            2 => PixelPrimitiveKind::Triangle,
            3 => PixelPrimitiveKind::Rectangle,
            4 => PixelPrimitiveKind::Flush,
            value => PixelPrimitiveKind::Invalid(value),
        };
        let features = PixelFeatureSet::new(draw_mode);
        let stipple_mode = PixelStippleMode::decode(snapshot.register32(0x0c0));
        let vertices_subpixel = if features.gl() {
            core::array::from_fn(|index| snapshot.gl_vertex(index))
        } else {
            core::array::from_fn(|index| {
                let (x, y) = snapshot.x_vertex(index);
                (i32::from(x) << 6, i32::from(y) << 6)
            })
        };
        let [
            (x0_subpixel, y0_subpixel),
            (x1_subpixel, y1_subpixel),
            (x2_subpixel, y2_subpixel),
        ] = vertices_subpixel;
        let to_pixel = |value: i32| value.div_euclid(64).clamp(0, i32::from(u16::MAX)) as u16;
        let (x0, y0) = (to_pixel(x0_subpixel), to_pixel(y0_subpixel));
        let (x1, y1) = (to_pixel(x1_subpixel), to_pixel(y1_subpixel));
        let (x2, y2) = (to_pixel(x2_subpixel), to_pixel(y2_subpixel));
        let decoded = DecodedPixelCommand {
            primitive_kind,
            primitive_raw,
            line_width: primitive_raw as u16,
            edge_type: ((primitive_raw >> 16) & 3) as u8,
            skip_last_endpoint: primitive_raw & (1 << 18) != 0,
            features,
            source,
            destination,
            stipple_mode,
            stipple_pattern: snapshot.register32(0x0c4),
            foreground_color: snapshot.foreground_color(),
            background_color: snapshot.background_color(),
            clip_mode: snapshot.clip_mode(),
            screen_masks: core::array::from_fn(|index| {
                PixelRectangle::decode(snapshot.register64(0x020 + index as u64 * 8))
            }),
            scissor: PixelRectangle::decode(snapshot.register64(0x048)),
            source_window_offset: {
                let value = snapshot.source_window_offset();
                ((value >> 16) as u16, value as u16)
            },
            destination_window_offset: {
                let value = snapshot.destination_window_offset();
                ((value >> 16) as u16, value as u16)
            },
            x0,
            y0,
            x1,
            y1,
            x2,
            y2,
            vertices_subpixel,
            snapshot,
        };
        let mut validation = Self {
            decoded,
            violations: Vec::new(),
        };
        validation.validate_encoding();
        validation.violations.sort_by_key(|violation| {
            (
                violation.kind,
                violation.register,
                violation.field,
                violation.value,
            )
        });
        validation.violations.dedup();
        validation
    }

    fn validate_encoding(&mut self) {
        let command = &self.decoded;
        add_reserved(
            &mut self.violations,
            PixelRegister::DrawMode,
            u64::from(command.snapshot.draw_mode() & !DRAW_MODE_DEFINED_MASK),
        );
        add_reserved(
            &mut self.violations,
            PixelRegister::Primitive,
            u64::from(command.primitive_raw & PRIMITIVE_RESERVED_MASK),
        );
        if let PixelPrimitiveKind::Invalid(value) = command.primitive_kind {
            add_violation(
                &mut self.violations,
                PixelRegister::Primitive,
                PixelField::PrimitiveOpcode,
                u64::from(value),
                PixelViolationKind::InvalidEncoding,
            );
        }
        validate_buffer_mode(
            &mut self.violations,
            PixelRegister::SourceBufferMode,
            command.source,
            false,
        );
        validate_buffer_mode(
            &mut self.violations,
            PixelRegister::DestinationBufferMode,
            command.destination,
            true,
        );
        add_reserved(
            &mut self.violations,
            PixelRegister::ClipMode,
            u64::from(command.snapshot.clip_mode() & CLIP_MODE_RESERVED_MASK),
        );

        if command.line_stipple_enabled() || command.features.enabled(FEATURE_POLYGON_STIPPLE) {
            if command.stipple_mode.index > 31 {
                add_violation(
                    &mut self.violations,
                    PixelRegister::StippleMode,
                    PixelField::StippleIndex,
                    u64::from(command.stipple_mode.index),
                    PixelViolationKind::InvalidEncoding,
                );
            }
            if command.stipple_mode.max_index > 31 {
                add_violation(
                    &mut self.violations,
                    PixelRegister::StippleMode,
                    PixelField::StippleMaxIndex,
                    u64::from(command.stipple_mode.max_index),
                    PixelViolationKind::InvalidEncoding,
                );
            }
        }

        if command.features.enabled(FEATURE_TEXTURE) {
            validate_texture_mode(&command.snapshot, &mut self.violations);
        }
        if command.features.enabled(FEATURE_ALPHA_TEST) {
            validate_alpha_test(&command.snapshot, &mut self.violations);
        }
        if command.features.enabled(FEATURE_BLEND) {
            validate_blend(&command.snapshot, &mut self.violations);
        }
        if command.features.enabled(FEATURE_LOGIC_OPERATION) {
            let raw = command.snapshot.logic_operation();
            add_reserved(
                &mut self.violations,
                PixelRegister::LogicOperation,
                u64::from(raw & !0x0f),
            );
        }
        if command.features.enabled(FEATURE_DEPTH_TEST)
            || command.features.enabled(FEATURE_DEPTH_MASK)
        {
            let raw = command.snapshot.register32(0x1c0);
            add_reserved(
                &mut self.violations,
                PixelRegister::DepthMode,
                u64::from(raw & 0xf000_0000),
            );
        }
        if command.features.enabled(FEATURE_STENCIL_TEST) {
            validate_stencil(&command.snapshot, &mut self.violations);
        }
    }
}

fn decode_buffer_mode(raw: u32) -> PixelBufferMode {
    let kind_raw = ((raw >> 10) & 7) as u8;
    let buffer_depth = ((raw >> 8) & 3) as u8;
    let pixel_type = ((raw >> 4) & 0x0f) as u8;
    let pixel_depth = ((raw >> 2) & 3) as u8;
    let bits = match pixel_depth {
        0 => 8,
        1 => 16,
        2 => 32,
        _ => 0,
    };
    let kind = match kind_raw {
        0 => PixelBufferKind::FramebufferA,
        1 => PixelBufferKind::FramebufferB,
        2 => PixelBufferKind::FramebufferC,
        3 => PixelBufferKind::Invalid(3),
        4 => PixelBufferKind::LinearA,
        5 => PixelBufferKind::LinearB,
        6 => PixelBufferKind::Invalid(6),
        value => PixelBufferKind::Invalid(value),
    };
    let format = match (pixel_type, pixel_depth) {
        (0, 0..=2) => PixelFormat::ColorIndex(bits),
        (1, 0..=2) => PixelFormat::Rgb(bits),
        (2, 0..=2) => PixelFormat::Rgba(bits),
        (3, 0..=2) => PixelFormat::Abgr(bits),
        (15, 0..=2) => PixelFormat::YCrCb(bits),
        _ => PixelFormat::Invalid {
            pixel_type,
            pixel_depth,
        },
    };
    PixelBufferMode {
        raw,
        kind,
        buffer_depth,
        format,
        pixel_depth,
        double_pixel: raw & 2 != 0,
        double_pixel_select: raw & 1 != 0,
    }
}

fn validate_buffer_mode(
    violations: &mut Vec<PixelCommandViolation>,
    register: PixelRegister,
    mode: PixelBufferMode,
    destination: bool,
) {
    add_reserved(
        violations,
        register,
        u64::from(mode.raw & BUFFER_MODE_RESERVED_MASK),
    );
    if let PixelBufferKind::Invalid(value) = mode.kind {
        add_violation(
            violations,
            register,
            PixelField::BufferKind,
            u64::from(value),
            PixelViolationKind::InvalidEncoding,
        );
    }
    if mode.buffer_depth == 3 {
        add_violation(
            violations,
            register,
            PixelField::BufferDepth,
            3,
            PixelViolationKind::InvalidEncoding,
        );
    }
    if let PixelFormat::Invalid {
        pixel_type,
        pixel_depth,
    } = mode.format
    {
        if !matches!(pixel_type, 0 | 1 | 2 | 3 | 15) {
            add_violation(
                violations,
                register,
                PixelField::PixelType,
                u64::from(pixel_type),
                PixelViolationKind::InvalidEncoding,
            );
        }
        if pixel_depth == 3 {
            add_violation(
                violations,
                register,
                PixelField::PixelDepth,
                3,
                PixelViolationKind::InvalidEncoding,
            );
        }
    }
    if mode.buffer_depth < mode.pixel_depth && mode.buffer_depth != 3 && mode.pixel_depth != 3 {
        add_violation(
            violations,
            register,
            PixelField::BufferDepth,
            u64::from(mode.raw),
            PixelViolationKind::InvalidCombination,
        );
    }
    if mode.double_pixel
        && mode.buffer_depth != 3
        && mode.pixel_depth != 3
        && mode.buffer_depth != mode.pixel_depth + 1
    {
        add_violation(
            violations,
            register,
            PixelField::BufferDepth,
            u64::from(mode.raw),
            PixelViolationKind::InvalidCombination,
        );
    }
    if destination && matches!(mode.format, PixelFormat::YCrCb(_)) {
        add_violation(
            violations,
            register,
            PixelField::PixelType,
            15,
            PixelViolationKind::InvalidCombination,
        );
    }
}

fn validate_texture_mode(
    snapshot: &PixelCommandSnapshot,
    violations: &mut Vec<PixelCommandViolation>,
) {
    let raw = snapshot.register32(0x110);
    add_reserved(
        violations,
        PixelRegister::TextureMode,
        u64::from(raw & 0xfe00_0000),
    );
    let texel_type = (raw >> 20) & 0x0f;
    if !matches!(texel_type, 1 | 2 | 4..=8) {
        add_violation(
            violations,
            PixelRegister::TextureMode,
            PixelField::TexelType,
            u64::from(texel_type),
            PixelViolationKind::InvalidEncoding,
        );
    }
    let texel_depth = (raw >> 18) & 3;
    if !matches!(texel_depth, 1 | 2) {
        add_violation(
            violations,
            PixelRegister::TextureMode,
            PixelField::TexelDepth,
            u64::from(texel_depth),
            PixelViolationKind::InvalidEncoding,
        );
    }
    let format = snapshot.register64(0x118);
    add_reserved(
        violations,
        PixelRegister::TextureFormat,
        format & 0xffff_0000_0000_0000,
    );
    let base_level = (raw >> 14) & 0x0f;
    let max_level = (raw >> 10) & 0x0f;
    if base_level > max_level {
        add_violation(
            violations,
            PixelRegister::TextureMode,
            PixelField::TextureBaseLevel,
            u64::from(base_level),
            PixelViolationKind::InvalidCombination,
        );
    }
    let min_filter = (raw >> 7) & 7;
    if min_filter > 5 {
        add_violation(
            violations,
            PixelRegister::TextureMode,
            PixelField::TextureMinificationFilter,
            u64::from(min_filter),
            PixelViolationKind::InvalidEncoding,
        );
    }
}

fn validate_alpha_test(
    snapshot: &PixelCommandSnapshot,
    violations: &mut Vec<PixelCommandViolation>,
) {
    let raw = snapshot.register32(0x198);
    add_reserved(
        violations,
        PixelRegister::AlphaTest,
        u64::from(raw & 0xffff_f000),
    );
    let function = (raw >> 8) & 0x0f;
    if function > 7 {
        add_violation(
            violations,
            PixelRegister::AlphaTest,
            PixelField::AlphaFunction,
            u64::from(function),
            PixelViolationKind::InvalidEncoding,
        );
    }
}

fn validate_blend(snapshot: &PixelCommandSnapshot, violations: &mut Vec<PixelCommandViolation>) {
    let raw = snapshot.register32(0x1a8);
    add_reserved(
        violations,
        PixelRegister::BlendFunction,
        u64::from(raw & 0xffff_f000),
    );
    for (field, value, limit) in [
        (PixelField::BlendOperation, (raw >> 8) & 0x0f, 4),
        (PixelField::SourceBlendFactor, (raw >> 4) & 0x0f, 12),
        (PixelField::DestinationBlendFactor, raw & 0x0f, 11),
    ] {
        if value > limit {
            add_violation(
                violations,
                PixelRegister::BlendFunction,
                field,
                u64::from(value),
                PixelViolationKind::InvalidEncoding,
            );
        }
    }
}

fn validate_stencil(snapshot: &PixelCommandSnapshot, violations: &mut Vec<PixelCommandViolation>) {
    let raw = snapshot.register32(0x1e0);
    let function = (raw >> 12) & 0x0f;
    if function > 7 {
        add_violation(
            violations,
            PixelRegister::StencilMode,
            PixelField::StencilFunction,
            u64::from(function),
            PixelViolationKind::InvalidEncoding,
        );
    }
    for operation in [(raw >> 8) & 0x0f, (raw >> 4) & 0x0f, raw & 0x0f] {
        if operation > 5 {
            add_violation(
                violations,
                PixelRegister::StencilMode,
                PixelField::StencilOperation,
                u64::from(operation),
                PixelViolationKind::InvalidEncoding,
            );
        }
    }
}

fn add_reserved(violations: &mut Vec<PixelCommandViolation>, register: PixelRegister, value: u64) {
    if value != 0 {
        add_violation(
            violations,
            register,
            PixelField::Reserved,
            value,
            PixelViolationKind::ReservedBits,
        );
    }
}

fn add_violation(
    violations: &mut Vec<PixelCommandViolation>,
    register: PixelRegister,
    field: PixelField,
    value: u64,
    kind: PixelViolationKind,
) {
    violations.push(PixelCommandViolation {
        register,
        field,
        value,
        kind,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(primitive: u32, draw_mode: u32) -> PixelCommandSnapshot {
        let mut registers = PixelRegisters::new();
        registers.write(PIXEL_PIPE_BASE + 0x060, 4, primitive.into());
        registers.write(PIXEL_PIPE_BASE + 0x018, 4, draw_mode.into());
        registers.command_snapshot(PIXEL_PIPE_NULL)
    }

    #[test]
    fn every_low_draw_mode_bit_is_defined() {
        for bit in 0..24 {
            let validation = PixelCommandValidation::decode(snapshot(0, 1 << bit));
            assert!(
                validation
                    .violations
                    .iter()
                    .all(|violation| violation.register != PixelRegister::DrawMode),
                "DrawMode bit {bit} was classified as invalid"
            );
            assert_eq!(validation.decoded.features.raw(), 1 << bit);
        }
    }

    #[test]
    fn dither_and_logic_bits_follow_the_register_specification() {
        let dither = PixelCommandValidation::decode(snapshot(0, 1 << 9));
        assert!(dither.decoded.features.dither());
        assert!(!dither.decoded.features.logic());

        let logic = PixelCommandValidation::decode(snapshot(0, 1 << 8));
        assert!(logic.decoded.features.logic());
        assert!(!logic.decoded.features.dither());
    }

    #[test]
    fn high_draw_mode_bits_are_reserved() {
        let validation = PixelCommandValidation::decode(snapshot(0, 0x8000_0000));
        assert!(validation.violations.contains(&PixelCommandViolation {
            register: PixelRegister::DrawMode,
            field: PixelField::Reserved,
            value: 0x8000_0000,
            kind: PixelViolationKind::ReservedBits,
        }));
    }

    #[test]
    fn disabled_fragment_registers_do_not_create_violations() {
        let mut registers = PixelRegisters::new();
        for offset in [0x110, 0x198, 0x1a8, 0x1b0, 0x1c0, 0x1e0] {
            registers.write(PIXEL_PIPE_BASE + offset, 4, u32::MAX.into());
        }
        let validation =
            PixelCommandValidation::decode(registers.command_snapshot(PIXEL_PIPE_NULL));
        assert!(validation.violations.is_empty());
    }

    #[test]
    fn flush_opcode_is_legal_and_has_no_capability_blocker() {
        let validation = PixelCommandValidation::decode(snapshot(0x0400_0000, 0x78));
        assert!(validation.violations.is_empty());
    }

    #[test]
    fn primitive_opcodes_are_classified_without_whole_word_matching() {
        for opcode in 0..=3_u32 {
            let validation = PixelCommandValidation::decode(snapshot(opcode << 24, 0x78));
            assert!(
                validation
                    .violations
                    .iter()
                    .all(|violation| { violation.field != PixelField::PrimitiveOpcode })
            );
        }
        let invalid = PixelCommandValidation::decode(snapshot(5 << 24, 0x78));
        assert!(invalid.violations.contains(&PixelCommandViolation {
            register: PixelRegister::Primitive,
            field: PixelField::PrimitiveOpcode,
            value: 5,
            kind: PixelViolationKind::InvalidEncoding,
        }));
    }

    #[test]
    fn buffer_mode_enumerations_are_decoded_field_by_field() {
        for kind in [0_u32, 1, 2, 4, 5] {
            for depth in 0..=2_u32 {
                for pixel_type in [0_u32, 1, 2, 3] {
                    let raw = kind << 10 | depth << 8 | pixel_type << 4 | depth << 2;
                    let mode = decode_buffer_mode(raw);
                    let mut violations = Vec::new();
                    validate_buffer_mode(
                        &mut violations,
                        PixelRegister::SourceBufferMode,
                        mode,
                        false,
                    );
                    assert!(violations.is_empty(), "BufMode {raw:#010x}");
                }
            }
        }
        for kind in [3_u32, 6] {
            let mut violations = Vec::new();
            validate_buffer_mode(
                &mut violations,
                PixelRegister::SourceBufferMode,
                decode_buffer_mode(kind << 10),
                false,
            );
            assert!(violations.iter().any(|violation| {
                violation.field == PixelField::BufferKind
                    && violation.kind == PixelViolationKind::InvalidEncoding
            }));
        }
        let mut violations = Vec::new();
        validate_buffer_mode(
            &mut violations,
            PixelRegister::SourceBufferMode,
            decode_buffer_mode(7 << 10 | 3 << 8 | 4 << 4 | 3 << 2),
            false,
        );
        assert!(violations.len() >= 4);
    }

    #[test]
    fn enabled_fragment_registers_accept_all_documented_maximum_enumerations() {
        let mut registers = PixelRegisters::new();
        registers.write(PIXEL_PIPE_BASE + 0x018, 4, 0x0000_8e7d);
        registers.write(PIXEL_PIPE_BASE + 0x110, 4, 0x008a_aaa0);
        registers.write(PIXEL_PIPE_BASE + 0x198, 4, 7 << 8);
        registers.write(PIXEL_PIPE_BASE + 0x1a8, 4, 4 << 8 | 12 << 4 | 11);
        registers.write(PIXEL_PIPE_BASE + 0x1b0, 4, 15);
        registers.write(PIXEL_PIPE_BASE + 0x1c0, 4, 7 << 25);
        registers.write(PIXEL_PIPE_BASE + 0x1e0, 4, 7 << 12 | 5 << 8 | 5 << 4 | 5);
        let validation =
            PixelCommandValidation::decode(registers.command_snapshot(PIXEL_PIPE_NULL));
        assert!(
            validation.violations.is_empty(),
            "{:?}",
            validation.violations
        );
    }

    #[test]
    fn invalid_fields_are_reported_in_stable_order() {
        let mut registers = PixelRegisters::new();
        registers.write(PIXEL_PIPE_BASE, 4, 0xffff_ffff);
        registers.write(PIXEL_PIPE_BASE + 0x008, 4, 0xffff_ffff);
        registers.write(PIXEL_PIPE_BASE + 0x018, 4, 0xffc0_0c00);
        registers.write(PIXEL_PIPE_BASE + 0x060, 4, 0xfff8_0001);
        registers.write(PIXEL_PIPE_BASE + 0x110, 4, 0xffff_ffff);
        registers.write(PIXEL_PIPE_BASE + 0x198, 4, 0xffff_ffff);
        registers.write(PIXEL_PIPE_BASE + 0x1a8, 4, 0xffff_ffff);
        let first = PixelCommandValidation::decode(registers.command_snapshot(PIXEL_PIPE_NULL));
        let second = PixelCommandValidation::decode(first.decoded.snapshot.clone());
        assert!(first.violations.len() > 4);
        assert_eq!(first.violations, second.violations);
    }
}
