//! PixelPipe command snapshots, typed decoding, and aggregate validation.

use super::super::{
    FRAMEBUFFER_8_WIDTH, FRAMEBUFFER_32_WIDTH, FRAMEBUFFER_HEIGHT, LOGIC_COPY, PIXEL_PIPE_BASE,
    PIXEL_PIPE_NULL, PixelBlockerKind, PixelCapability, PixelCommandBlocker, PixelCommandViolation,
    PixelField, PixelRegister, PixelViolationKind, read_register_slot, write_register_slot,
};
use super::stipple::PixelStippleMode;

const DRAW_MODE_DEFINED_MASK: u32 = 0x00ff_ffff;
const PRIMITIVE_RESERVED_MASK: u32 = 0x00f8_0000;
const BUFFER_MODE_RESERVED_MASK: u32 = 0xffff_e000;
const CLIP_MODE_RESERVED_MASK: u32 = 0xffff_f000;

const FEATURE_NO_CONFLICT: u8 = 23;
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
const FEATURE_LOGIC_OPERATION: u8 = 9;
const FEATURE_DITHER: u8 = 8;
const FEATURE_COLOR_MASK: u8 = 7;
const FEATURE_DEPTH_TEST: u8 = 2;
const FEATURE_DEPTH_MASK: u8 = 1;
const FEATURE_STENCIL_TEST: u8 = 0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PixelFeatureEvidence {
    A,
    B,
    U,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PixelFeatureDescriptor {
    bit: u8,
    evidence: PixelFeatureEvidence,
    capability: Option<PixelCapability>,
}

const PIXEL_FEATURE_DESCRIPTORS: [PixelFeatureDescriptor; 24] = [
    feature(
        FEATURE_NO_CONFLICT,
        PixelFeatureEvidence::A,
        Some(PixelCapability::ConflictBypass),
    ),
    feature(
        FEATURE_GL,
        PixelFeatureEvidence::A,
        Some(PixelCapability::GlRasterization),
    ),
    feature(FEATURE_PIXEL_TRANSFER, PixelFeatureEvidence::A, None),
    feature(
        FEATURE_SCISSOR,
        PixelFeatureEvidence::A,
        Some(PixelCapability::ScissorTest),
    ),
    feature(FEATURE_LINE_STIPPLE, PixelFeatureEvidence::A, None),
    feature(
        FEATURE_POLYGON_STIPPLE,
        PixelFeatureEvidence::A,
        Some(PixelCapability::PolygonStipple),
    ),
    feature(
        FEATURE_OPAQUE_STIPPLE,
        PixelFeatureEvidence::U,
        Some(PixelCapability::OpaqueStipple),
    ),
    feature(
        FEATURE_SHADE,
        PixelFeatureEvidence::A,
        Some(PixelCapability::SmoothShade),
    ),
    feature(
        FEATURE_TEXTURE,
        PixelFeatureEvidence::A,
        Some(PixelCapability::TextureMapping),
    ),
    feature(
        FEATURE_FOG,
        PixelFeatureEvidence::A,
        Some(PixelCapability::Fog),
    ),
    feature(
        FEATURE_COVERAGE,
        PixelFeatureEvidence::B,
        Some(PixelCapability::Coverage),
    ),
    feature(
        FEATURE_ANTIALIAS_LINE,
        PixelFeatureEvidence::U,
        Some(PixelCapability::LineAntialiasing),
    ),
    feature(
        FEATURE_ALPHA_TEST,
        PixelFeatureEvidence::A,
        Some(PixelCapability::AlphaTest),
    ),
    feature(
        FEATURE_BLEND,
        PixelFeatureEvidence::A,
        Some(PixelCapability::Blend),
    ),
    feature(FEATURE_LOGIC_OPERATION, PixelFeatureEvidence::B, None),
    feature(
        FEATURE_DITHER,
        PixelFeatureEvidence::U,
        Some(PixelCapability::Dither),
    ),
    feature(FEATURE_COLOR_MASK, PixelFeatureEvidence::A, None),
    feature(6, PixelFeatureEvidence::A, None),
    feature(5, PixelFeatureEvidence::A, None),
    feature(4, PixelFeatureEvidence::A, None),
    feature(3, PixelFeatureEvidence::A, None),
    feature(
        FEATURE_DEPTH_TEST,
        PixelFeatureEvidence::B,
        Some(PixelCapability::Depth),
    ),
    feature(
        FEATURE_DEPTH_MASK,
        PixelFeatureEvidence::B,
        Some(PixelCapability::Depth),
    ),
    feature(
        FEATURE_STENCIL_TEST,
        PixelFeatureEvidence::B,
        Some(PixelCapability::Stencil),
    ),
];

const fn feature(
    bit: u8,
    evidence: PixelFeatureEvidence,
    capability: Option<PixelCapability>,
) -> PixelFeatureDescriptor {
    PixelFeatureDescriptor {
        bit,
        evidence,
        capability,
    }
}

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

    pub(crate) const fn supported_for_flat_write(self) -> bool {
        matches!(self, Self::ColorIndex(8) | Self::Rgba(32))
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
    pub(crate) x0: u16,
    pub(crate) y0: u16,
    pub(crate) x1: u16,
    pub(crate) y1: u16,
}

impl DecodedPixelCommand {
    pub(crate) fn endpoints(&self) -> (u16, u16, u16, u16) {
        (self.x0, self.y0, self.x1, self.y1)
    }

    pub(crate) fn line_stipple_enabled(&self) -> bool {
        self.features.enabled(FEATURE_LINE_STIPPLE)
    }

    pub(crate) fn pixel_bytes(&self) -> [u8; 4] {
        match self.destination.format {
            PixelFormat::ColorIndex(8) => [self.foreground_color as u8, 0, 0, 0],
            PixelFormat::Rgba(32) => self.foreground_color.to_be_bytes(),
            _ => unreachable!("validated PixelPipe command has a supported pixel format"),
        }
    }
}

/// Aggregate result of decoding and validating one command snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PixelCommandValidation {
    pub(crate) decoded: DecodedPixelCommand,
    pub(crate) violations: Vec<PixelCommandViolation>,
    pub(crate) blockers: Vec<PixelCommandBlocker>,
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
        let (x0, y0) = snapshot.x_vertex(0);
        let (x1, y1) = snapshot.x_vertex(1);
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
            x0,
            y0,
            x1,
            y1,
            snapshot,
        };
        let mut validation = Self {
            decoded,
            violations: Vec::new(),
            blockers: Vec::new(),
        };
        validation.validate_encoding();
        validation.validate_enabled_state();
        validation.validate_capabilities();
        validation.violations.sort_by_key(|violation| {
            (
                violation.kind,
                violation.register,
                violation.field,
                violation.value,
            )
        });
        validation.violations.dedup();
        validation.blockers.sort_by_key(|blocker| {
            (
                blocker.kind,
                blocker.capability,
                blocker.register,
                blocker.value,
            )
        });
        validation.blockers.dedup();
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
            validate_texture_mode(&command.snapshot, &mut self.violations, &mut self.blockers);
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

    fn validate_enabled_state(&mut self) {
        let command = &self.decoded;
        let features = command.features;
        let clip_mode = command.snapshot.clip_mode();
        for descriptor in PIXEL_FEATURE_DESCRIPTORS {
            if features.enabled(descriptor.bit) {
                let Some(capability) = descriptor.capability else {
                    continue;
                };
                add_blocker(
                    &mut self.blockers,
                    capability,
                    PixelRegister::DrawMode,
                    u64::from(features.raw()),
                    if descriptor.evidence == PixelFeatureEvidence::U {
                        PixelBlockerKind::Evidence
                    } else {
                        PixelBlockerKind::Implementation
                    },
                );
            }
        }
        if clip_mode & (1 << 11) != 0 {
            add_blocker(
                &mut self.blockers,
                PixelCapability::ClipIdTest,
                PixelRegister::ClipMode,
                u64::from(clip_mode),
                PixelBlockerKind::Implementation,
            );
        }
        if clip_mode & 0x03e0 != 0 {
            add_blocker(
                &mut self.blockers,
                PixelCapability::ScreenMaskTest,
                PixelRegister::ClipMode,
                u64::from(clip_mode),
                PixelBlockerKind::Implementation,
            );
        }
        if command.line_stipple_enabled() && clip_mode & 0x0be0 != 0 {
            add_blocker(
                &mut self.blockers,
                PixelCapability::ClippedLineStipple,
                PixelRegister::ClipMode,
                u64::from(clip_mode),
                PixelBlockerKind::Evidence,
            );
        }
        if features.enabled(FEATURE_LOGIC_OPERATION) {
            let logic = command.snapshot.logic_operation() & 0x0f;
            if logic != LOGIC_COPY {
                add_blocker(
                    &mut self.blockers,
                    PixelCapability::LogicReadModifyWrite,
                    PixelRegister::LogicOperation,
                    u64::from(logic),
                    PixelBlockerKind::Implementation,
                );
            }
        }
        if features.enabled(FEATURE_COLOR_MASK) && command.snapshot.color_mask() != u32::MAX {
            add_blocker(
                &mut self.blockers,
                PixelCapability::PartialColorMask,
                PixelRegister::ColorMask,
                u64::from(command.snapshot.color_mask()),
                PixelBlockerKind::Implementation,
            );
        }
        if features.color_byte_mask() != 0x0f {
            add_blocker(
                &mut self.blockers,
                PixelCapability::PartialColorByteMask,
                PixelRegister::DrawMode,
                u64::from(features.color_byte_mask()),
                PixelBlockerKind::Implementation,
            );
        }
        if command.snapshot.destination_window_offset() != 0 {
            add_blocker(
                &mut self.blockers,
                PixelCapability::WindowOffset,
                PixelRegister::DestinationWindowOffset,
                u64::from(command.snapshot.destination_window_offset()),
                PixelBlockerKind::Implementation,
            );
        }
    }

    fn validate_capabilities(&mut self) {
        let command = &self.decoded;
        if command.snapshot.trigger_address != PIXEL_PIPE_NULL {
            add_blocker(
                &mut self.blockers,
                PixelCapability::StartAlias,
                PixelRegister::StartTrigger,
                command.snapshot.trigger_address,
                PixelBlockerKind::Implementation,
            );
        }
        if command.source.double_pixel || command.destination.double_pixel {
            add_blocker(
                &mut self.blockers,
                PixelCapability::DoublePixel,
                PixelRegister::DestinationBufferMode,
                u64::from(command.destination.raw),
                PixelBlockerKind::Implementation,
            );
        }
        let pixel_transfer = command.features.pixel_transfer();
        if if pixel_transfer {
            command.source.kind.buffer_selector().is_none()
                || command.destination.kind.buffer_selector().is_none()
        } else {
            command.source.kind.framebuffer_selector().is_none()
                || command.destination.kind.framebuffer_selector().is_none()
        } {
            add_blocker(
                &mut self.blockers,
                PixelCapability::BufferKind,
                PixelRegister::DestinationBufferMode,
                u64::from(command.destination.raw),
                PixelBlockerKind::Implementation,
            );
        }
        if !pixel_transfer
            && (!command.source.format.supported_for_flat_write()
                || !command.destination.format.supported_for_flat_write())
        {
            add_blocker(
                &mut self.blockers,
                PixelCapability::PixelFormat,
                PixelRegister::DestinationBufferMode,
                u64::from(command.destination.raw),
                PixelBlockerKind::Implementation,
            );
        }
        if !pixel_transfer
            && (command.source.kind != command.destination.kind
                || command.source.format != command.destination.format
                || command.source.buffer_depth != command.destination.buffer_depth)
        {
            add_blocker(
                &mut self.blockers,
                PixelCapability::BufferConversion,
                PixelRegister::DestinationBufferMode,
                u64::from(command.destination.raw),
                PixelBlockerKind::Implementation,
            );
        }

        match command.primitive_kind {
            PixelPrimitiveKind::Point => add_blocker(
                &mut self.blockers,
                PixelCapability::PointRasterization,
                PixelRegister::Primitive,
                u64::from(command.primitive_raw),
                PixelBlockerKind::Implementation,
            ),
            PixelPrimitiveKind::Triangle => add_blocker(
                &mut self.blockers,
                PixelCapability::TriangleRasterization,
                PixelRegister::Primitive,
                u64::from(command.primitive_raw),
                PixelBlockerKind::Implementation,
            ),
            PixelPrimitiveKind::Flush => add_blocker(
                &mut self.blockers,
                PixelCapability::FlushPrimitive,
                PixelRegister::Primitive,
                u64::from(command.primitive_raw),
                PixelBlockerKind::Evidence,
            ),
            PixelPrimitiveKind::Line => self.validate_line_capabilities(),
            PixelPrimitiveKind::Rectangle => self.validate_rectangle_capabilities(),
            PixelPrimitiveKind::Invalid(_) => {}
        }
    }

    fn validate_line_capabilities(&mut self) {
        let command = &self.decoded;
        if command.line_width != 0x20 {
            add_blocker(
                &mut self.blockers,
                PixelCapability::GeneralLineWidth,
                PixelRegister::Primitive,
                u64::from(command.line_width),
                PixelBlockerKind::Implementation,
            );
        }
        if command.skip_last_endpoint {
            add_blocker(
                &mut self.blockers,
                PixelCapability::SkipLastEndpoint,
                PixelRegister::Primitive,
                u64::from(command.primitive_raw),
                PixelBlockerKind::Implementation,
            );
        }
        let horizontal = command.y0 == command.y1 && command.x0 <= command.x1;
        let vertical = command.x0 == command.x1 && command.y0 < command.y1;
        if command.edge_type != 0 || !horizontal && !vertical {
            add_blocker(
                &mut self.blockers,
                PixelCapability::GeneralLineRasterization,
                PixelRegister::XVertex,
                pack_endpoints(command),
                PixelBlockerKind::Implementation,
            );
        }
        if command.line_stipple_enabled() {
            if !horizontal {
                add_blocker(
                    &mut self.blockers,
                    PixelCapability::GeneralLineStipple,
                    PixelRegister::XVertex,
                    pack_endpoints(command),
                    PixelBlockerKind::Evidence,
                );
            }
            let candidate_count = u32::from(command.x1).saturating_sub(u32::from(command.x0)) + 1;
            let last_index = u32::from(command.stipple_mode.index) + candidate_count - 1;
            if command.stipple_mode.repeat_count != 0
                || command.stipple_mode.max_repeat != 0
                || command.stipple_mode.index > command.stipple_mode.max_index
                || last_index > u32::from(command.stipple_mode.max_index)
            {
                add_blocker(
                    &mut self.blockers,
                    PixelCapability::RepeatingLineStipple,
                    PixelRegister::StippleMode,
                    u64::from(command.snapshot.register32(0x0c0)),
                    PixelBlockerKind::Evidence,
                );
            }
        }
        let width = match command.destination.format {
            PixelFormat::ColorIndex(8) => FRAMEBUFFER_8_WIDTH,
            PixelFormat::Rgba(32) => FRAMEBUFFER_32_WIDTH,
            _ => return,
        };
        if command.x0 >= width
            || command.x1 >= width
            || command.y0 >= FRAMEBUFFER_HEIGHT
            || command.y1 >= FRAMEBUFFER_HEIGHT
        {
            add_blocker(
                &mut self.blockers,
                PixelCapability::FramebufferBounds,
                PixelRegister::XVertex,
                pack_endpoints(command),
                PixelBlockerKind::Implementation,
            );
        }
    }

    fn validate_rectangle_capabilities(&mut self) {
        let command = &self.decoded;
        let supported_traversal = match command.edge_type {
            0 => command.y0 >= command.y1,
            2 => command.y0 <= command.y1,
            _ => false,
        };
        if command.line_width != 0 || !supported_traversal || command.skip_last_endpoint {
            add_blocker(
                &mut self.blockers,
                PixelCapability::GeneralRectangleTraversal,
                PixelRegister::Primitive,
                u64::from(command.primitive_raw),
                PixelBlockerKind::Implementation,
            );
        }
        if command.x0 > command.x1 {
            add_blocker(
                &mut self.blockers,
                PixelCapability::GeneralRectangleTraversal,
                PixelRegister::XVertex,
                pack_endpoints(command),
                PixelBlockerKind::Implementation,
            );
        }
        if !command.features.enabled(FEATURE_LOGIC_OPERATION) && command.foreground_color != 0 {
            add_blocker(
                &mut self.blockers,
                PixelCapability::ZeroRectangleColor,
                PixelRegister::ColorMask,
                u64::from(command.foreground_color),
                PixelBlockerKind::Evidence,
            );
        }
        let width = match command.destination.format {
            PixelFormat::ColorIndex(8) => FRAMEBUFFER_8_WIDTH,
            PixelFormat::Rgba(32) => FRAMEBUFFER_32_WIDTH,
            _ => return,
        };
        if command.x0 >= width
            || command.x1 >= width
            || command.y0 >= FRAMEBUFFER_HEIGHT
            || command.y1 >= FRAMEBUFFER_HEIGHT
        {
            add_blocker(
                &mut self.blockers,
                PixelCapability::FramebufferBounds,
                PixelRegister::XVertex,
                pack_endpoints(command),
                PixelBlockerKind::Implementation,
            );
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
    blockers: &mut Vec<PixelCommandBlocker>,
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
    if texel_depth == 3 {
        add_violation(
            violations,
            PixelRegister::TextureMode,
            PixelField::TexelDepth,
            u64::from(texel_depth),
            PixelViolationKind::InvalidEncoding,
        );
    }
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
    if base_level > 10 || max_level > 10 {
        add_blocker(
            blockers,
            PixelCapability::TextureLevelRange,
            PixelRegister::TextureMode,
            u64::from(raw),
            PixelBlockerKind::Evidence,
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

fn add_blocker(
    blockers: &mut Vec<PixelCommandBlocker>,
    capability: PixelCapability,
    register: PixelRegister,
    value: u64,
    kind: PixelBlockerKind,
) {
    blockers.push(PixelCommandBlocker {
        capability,
        register,
        value,
        kind,
    });
}

fn pack_endpoints(command: &DecodedPixelCommand) -> u64 {
    u64::from(command.x0) << 48
        | u64::from(command.y0) << 32
        | u64::from(command.x1) << 16
        | u64::from(command.y1)
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
        let mut described_bits = PIXEL_FEATURE_DESCRIPTORS.map(|descriptor| descriptor.bit);
        described_bits.sort_unstable();
        assert_eq!(described_bits, core::array::from_fn(|index| index as u8));
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
    fn flush_opcode_is_an_evidence_blocker() {
        let validation = PixelCommandValidation::decode(snapshot(0x0400_0000, 0x78));
        assert!(validation.blockers.contains(&PixelCommandBlocker {
            capability: PixelCapability::FlushPrimitive,
            register: PixelRegister::Primitive,
            value: 0x0400_0000,
            kind: PixelBlockerKind::Evidence,
        }));
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
        assert!(validation.blockers.len() >= 6);
    }

    #[test]
    fn invalid_fields_and_capabilities_are_reported_together_in_stable_order() {
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
        assert!(first.blockers.len() > 2);
        assert_eq!(first.violations, second.violations);
        assert_eq!(first.blockers, second.blockers);
    }
}
