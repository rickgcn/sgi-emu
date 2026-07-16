//! CRIME Rendering Engine register front end and evidence-backed transfer path.
use core::fmt;
use super::memory::framebuffer;
use super::protocol::{
    CrimeBusError, CrimeByteEnable, CrimeCompletionPayload, CrimeData, CrimeMemoryBankSelect,
    CrimeMemoryInhibitReason, CrimeMemoryOutcome, CrimeTransfer,
};
use super::registers;
mod pixel;
use pixel::command::{
    DecodedPixelCommand, PixelCommandSnapshot, PixelCommandValidation, PixelPrimitiveKind,
    PixelRegisters,
};
use pixel::raster::{
    AxisLineDirection, AxisLineRasterizer, InclusiveRectangleRasterizer, Rasterizer,
    RectangleRowDirection,
};
use pixel::stipple::PixelStippleCursor;
const INTERFACE_DATA_BASE: u64 = registers::CRIME_RENDER_BASE;
const INTERFACE_ADDRESS_BASE: u64 = registers::CRIME_RENDER_BASE + 0x200;
const INTERFACE_CONTROL: u64 = registers::CRIME_RENDER_BASE + 0x400;
const INTERFACE_RESET: u64 = registers::CRIME_RENDER_BASE + 0x408;
const TLB_BASE: u64 = registers::CRIME_RENDER_BASE + 0x1000;
const FRAMEBUFFER_A_BASE: u64 = TLB_BASE;
const FRAMEBUFFER_B_BASE: u64 = TLB_BASE + 0x200;
const FRAMEBUFFER_C_BASE: u64 = TLB_BASE + 0x400;
const TEXTURE_TLB_BASE: u64 = TLB_BASE + 0x600;
const CID_TLB_BASE: u64 = TLB_BASE + 0x6e0;
const LINEAR_A_BASE: u64 = TLB_BASE + 0x700;
const LINEAR_B_BASE: u64 = TLB_BASE + 0x780;
const PIXEL_PIPE_BASE: u64 = registers::CRIME_RENDER_BASE + 0x2000;
const PIXEL_PIPE_NULL: u64 = PIXEL_PIPE_BASE + 0x1f0;
const MTE_BASE: u64 = registers::CRIME_RENDER_BASE + 0x3000;
const STATUS_BASE: u64 = registers::CRIME_RENDER_BASE + 0x4000;
const SET_START_POINTER: u64 = STATUS_BASE + 0x008;
const START_OFFSET: u64 = 0x800;
const INTERFACE_CAPACITY: usize = 64;
const LINEAR_PAGE_COUNT: usize = 32;
const LINEAR_PAGE_SIZE: u64 = 4096;
const LINEAR_PAGE_MASK: u32 = 0x0007_ffff;
const MAX_MEMORY_CHUNK_BYTES: usize = 512;
const FRAMEBUFFER_TLB_ENTRY_COUNT: usize = 256;
const FRAMEBUFFER_TILES_PER_ROW: usize = 16;
const FRAMEBUFFER_TILE_HEIGHT: u16 = 128;
const FRAMEBUFFER_TILE_BYTES: u64 = 64 * 1024;
const FRAMEBUFFER_TILE_ROW_BYTES: u64 = 512;
const FRAMEBUFFER_TLB_VALID: u16 = 0x8000;
const FRAMEBUFFER_TLB_TILE_MASK: u16 = 0x7fff;
const RENDER_MEMORY_WORD_BYTES: usize = 32;
const FRAMEBUFFER_8_WIDTH: u16 = 8192;
const FRAMEBUFFER_32_WIDTH: u16 = 2048;
const FRAMEBUFFER_HEIGHT: u16 = 2048;
const LOGIC_COPY: u32 = 3;
const STATUS_IDLE: u32 = 0x1000_0000;
const STATUS_SETUP_IDLE: u32 = 0x0800_0000;
const STATUS_PIXEL_PIPE_IDLE: u32 = 0x0400_0000;
const STATUS_MTE_IDLE: u32 = 0x0200_0000;
const STATUS_LEVEL_SHIFT: u32 = 18;
const STATUS_READ_POINTER_SHIFT: u32 = 12;
const STATUS_WRITE_POINTER_SHIFT: u32 = 6;
const STATUS_START_POINTER_SHIFT: u32 = 0;
const INTERFACE_CONTROL_MASK: u32 = 0x0fff_ffff;
const INTERFACE_FULL_SHIFT: u32 = 21;
const INTERFACE_EMPTY_SHIFT: u32 = 14;
const INTERFACE_STALL_LEVEL_SHIFT: u32 = 7;
const INTERFACE_FIELD_MASK: u32 = 0x7f;
const ADDRESS_START: u64 = 1 << 45;
const ADDRESS_WMASK_SHIFT: u32 = 43;
const ADDRESS_PAGE_SHIFT: u32 = 40;
const ADDRESS_OFFSET_SHIFT: u32 = 32;
const ADDRESS_WMASK_UPPER: u64 = 1;
const ADDRESS_WMASK_LOWER: u64 = 2;
const ADDRESS_WMASK_DOUBLE: u64 = 3;
/// One host register write retained by the Rendering Engine interface buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct RenderRegisterWrite {
    /// Canonical register address without the start tag.
    pub address: u64,
    /// Register value.
    pub value: u64,
    /// Access width in bytes.
    pub size: u8,
    /// Whether this write commits the current operation.
    pub commit: bool,
}
/// PixelPipe register associated with a command diagnostic.
#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub enum PixelRegister {
    /// Source buffer mode.
    SourceBufferMode,
    /// Destination buffer mode.
    DestinationBufferMode,
    /// Clip mode.
    ClipMode,
    /// Draw mode.
    DrawMode,
    /// Destination window offset.
    DestinationWindowOffset,
    /// Primitive descriptor.
    Primitive,
    /// Line-stipple mode.
    StippleMode,
    /// Texture mode.
    TextureMode,
    /// Alpha-test mode.
    AlphaTest,
    /// Blend function.
    BlendFunction,
    /// Logic operation.
    LogicOperation,
    /// Color mask.
    ColorMask,
    /// Depth mode.
    DepthMode,
    /// Stencil mode.
    StencilMode,
    /// Register whose START alias submitted the command.
    StartTrigger,
    /// Frozen X vertices.
    XVertex,
}
/// Category of a proven-invalid PixelPipe command field.
#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub enum PixelViolationKind {
    /// Bits documented as reserved were nonzero.
    ReservedBits,
    /// An enumerated field used a reserved encoding.
    InvalidEncoding,
    /// Individually valid fields formed a documented invalid combination.
    InvalidCombination,
}
/// PixelPipe field associated with a command violation.
#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub enum PixelField {
    /// Reserved bits.
    Reserved,
    /// Primitive opcode.
    PrimitiveOpcode,
    /// Buffer selector.
    BufferKind,
    /// Buffer word depth.
    BufferDepth,
    /// Pixel type.
    PixelType,
    /// Pixel depth.
    PixelDepth,
    /// Stipple starting index.
    StippleIndex,
    /// Stipple maximum index.
    StippleMaxIndex,
    /// Texture texel type.
    TexelType,
    /// Texture texel depth.
    TexelDepth,
    /// Texture base level.
    TextureBaseLevel,
    /// Texture maximum level.
    TextureMaximumLevel,
    /// Texture minification filter.
    TextureMinificationFilter,
    /// Alpha comparison function.
    AlphaFunction,
    /// Blend equation.
    BlendOperation,
    /// Source blend factor.
    SourceBlendFactor,
    /// Destination blend factor.
    DestinationBlendFactor,
    /// Logic operation.
    LogicOperation,
    /// Depth comparison function.
    DepthFunction,
    /// Stencil comparison function.
    StencilFunction,
    /// Stencil operation.
    StencilOperation,
}
/// One proven-invalid field in a frozen PixelPipe command.
#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub struct PixelCommandViolation {
    /// Register containing the invalid field.
    pub register: PixelRegister,
    /// Field whose value was invalid.
    pub field: PixelField,
    /// Raw field or bit value.
    pub value: u64,
    /// Reason the value is invalid.
    pub kind: PixelViolationKind,
}
/// Evidence or implementation capability needed by a valid PixelPipe command.
#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub enum PixelCapability {
    /// Primitive opcode 4 semantics.
    FlushPrimitive,
    /// A START alias other than PixPipeNull.
    StartAlias,
    /// Point rasterization.
    PointRasterization,
    /// Triangle rasterization.
    TriangleRasterization,
    /// Non-axis-aligned or reverse X line rasterization.
    GeneralLineRasterization,
    /// Line width other than the proven X-line value.
    GeneralLineWidth,
    /// Endpoint skipping.
    SkipLastEndpoint,
    /// Rectangle traversal outside the evidence-backed left-to-right row modes.
    GeneralRectangleTraversal,
    /// Coordinate range not covered by the current framebuffer iterator.
    FramebufferBounds,
    /// GL vertex and raster semantics.
    GlRasterization,
    /// Pixel transfer.
    PixelTransfer,
    /// Scissor testing.
    ScissorTest,
    /// Screen-mask testing.
    ScreenMaskTest,
    /// Clip-ID testing.
    ClipIdTest,
    /// Polygon stippling.
    PolygonStipple,
    /// Opaque stippling.
    OpaqueStipple,
    /// Line-stipple repetition or index wrap.
    RepeatingLineStipple,
    /// Line stippling combined with clipping.
    ClippedLineStipple,
    /// Line stippling on a non-horizontal line.
    GeneralLineStipple,
    /// Smooth shading.
    SmoothShade,
    /// Texture mapping.
    TextureMapping,
    /// Texture levels beyond the evidence-backed range.
    TextureLevelRange,
    /// Fog.
    Fog,
    /// Coverage.
    Coverage,
    /// Line antialiasing.
    LineAntialiasing,
    /// Alpha testing.
    AlphaTest,
    /// Blending.
    Blend,
    /// RGB dithering.
    Dither,
    /// Destination read-modify-write for a non-COPY logic operation.
    LogicReadModifyWrite,
    /// Partial color bit masking.
    PartialColorMask,
    /// Partial color byte masking.
    PartialColorByteMask,
    /// Depth testing or writes.
    Depth,
    /// Stencil testing or writes.
    Stencil,
    /// Read/write conflict bypass.
    ConflictBypass,
    /// Double-pixel buffer selection.
    DoublePixel,
    /// A legal buffer kind not supported by the current raster path.
    BufferKind,
    /// A legal pixel format not supported by the current raster path.
    PixelFormat,
    /// Source and destination conversion.
    BufferConversion,
    /// Destination window translation.
    WindowOffset,
    /// PROM zero-fill rectangle with a nonzero flat color.
    ZeroRectangleColor,
}
/// Source of a capability blocker.
#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub enum PixelBlockerKind {
    /// Software-visible behavior is not yet evidence-complete.
    Evidence,
    /// Behavior is legal and evidence-complete but not implemented.
    Implementation,
}
/// One capability missing from an otherwise legal PixelPipe command.
#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub struct PixelCommandBlocker {
    /// Missing capability.
    pub capability: PixelCapability,
    /// Register that enabled or selected the capability.
    pub register: PixelRegister,
    /// Raw enabling value.
    pub value: u64,
    /// Whether evidence or implementation is missing.
    pub kind: PixelBlockerKind,
}
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct HostInterface {
    #[serde(with = "crate::common::serde_array")]
    data: [u64; INTERFACE_CAPACITY],
    #[serde(with = "crate::common::serde_array")]
    address: [u64; INTERFACE_CAPACITY],
    control: u32,
    read_pointer: u8,
    write_pointer: u8,
    start_pointer: u8,
    level: u8,
    stall_cycles: u8,
}
impl HostInterface {
    const fn new() -> Self {
        Self {
            data: [0; INTERFACE_CAPACITY],
            address: [0; INTERFACE_CAPACITY],
            control: 0,
            read_pointer: 0,
            write_pointer: 0,
            start_pointer: 0,
            level: 0,
            stall_cycles: 0,
        }
    }
    fn reset(&mut self) {
        self.data.fill(0);
        self.address.fill(0);
        self.read_pointer = 0;
        self.write_pointer = 0;
        self.start_pointer = 0;
        self.level = 0;
        self.stall_cycles = 0;
    }
    const fn level(&self) -> usize {
        self.level as usize
    }
    const fn can_accept(&self) -> bool {
        self.level < INTERFACE_CAPACITY as u8 && self.stall_cycles == 0
    }
    const fn empty_level(&self) -> u8 {
        ((self.control >> INTERFACE_EMPTY_SHIFT) & INTERFACE_FIELD_MASK) as u8
    }
    const fn full_level(&self) -> u8 {
        ((self.control >> INTERFACE_FULL_SHIFT) & INTERFACE_FIELD_MASK) as u8
    }
    const fn stall_level(&self) -> u8 {
        ((self.control >> INTERFACE_STALL_LEVEL_SHIFT) & INTERFACE_FIELD_MASK) as u8
    }
    const fn stall_count(&self) -> u8 {
        (self.control & INTERFACE_FIELD_MASK) as u8
    }
    const fn empty_condition(&self) -> bool {
        self.level <= self.empty_level()
    }
    const fn full_condition(&self) -> bool {
        self.full_level() != 0 && self.level >= self.full_level()
    }
    fn set_control(&mut self, value: u32) {
        self.control = value & INTERFACE_CONTROL_MASK;
    }
    fn set_start_pointer(&mut self) {
        self.start_pointer = self.write_pointer;
    }
    fn accept(&mut self, write: RenderRegisterWrite) {
        debug_assert!(self.can_accept());
        let index = usize::from(self.write_pointer);
        let (data, address) = encode_interface_entry(write);
        self.data[index] = data;
        self.address[index] = address;
        self.write_pointer = self.write_pointer.wrapping_add(1) & 0x3f;
        self.level += 1;
        if write.commit {
            self.set_start_pointer();
        }
        if self.level > self.stall_level() {
            self.stall_cycles = self.stall_count();
        }
    }
    fn retire(&mut self) -> Option<RenderRegisterWrite> {
        if self.level == 0 {
            return None;
        }
        let index = usize::from(self.read_pointer);
        let write = decode_interface_entry(self.data[index], self.address[index]);
        self.read_pointer = self.read_pointer.wrapping_add(1) & 0x3f;
        self.level -= 1;
        Some(write)
    }
    fn advance_stall(&mut self) {
        self.stall_cycles = self.stall_cycles.saturating_sub(1);
    }
    fn read_ram(&self, address: u64) -> Option<u64> {
        if (INTERFACE_DATA_BASE..INTERFACE_DATA_BASE + 0x200).contains(&address) {
            return Some(self.data[((address - INTERFACE_DATA_BASE) / 8) as usize]);
        }
        if (INTERFACE_ADDRESS_BASE..INTERFACE_ADDRESS_BASE + 0x200).contains(&address) {
            return Some(self.address[((address - INTERFACE_ADDRESS_BASE) / 8) as usize]);
        }
        None
    }
    fn write_ram(&mut self, address: u64, value: u64) -> bool {
        if (INTERFACE_DATA_BASE..INTERFACE_DATA_BASE + 0x200).contains(&address) {
            self.data[((address - INTERFACE_DATA_BASE) / 8) as usize] = value;
            return true;
        }
        if (INTERFACE_ADDRESS_BASE..INTERFACE_ADDRESS_BASE + 0x200).contains(&address) {
            self.address[((address - INTERFACE_ADDRESS_BASE) / 8) as usize] = value;
            return true;
        }
        false
    }
}
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct RenderTlbs {
    #[serde(with = "crate::common::serde_array")]
    framebuffer_a: [u64; 64],
    #[serde(with = "crate::common::serde_array")]
    framebuffer_b: [u64; 64],
    #[serde(with = "crate::common::serde_array")]
    framebuffer_c: [u64; 64],
    texture: [u64; 28],
    cid: [u64; 4],
    linear_a: [u64; 16],
    linear_b: [u64; 16],
}
impl RenderTlbs {
    const fn new() -> Self {
        Self {
            framebuffer_a: [0; 64],
            framebuffer_b: [0; 64],
            framebuffer_c: [0; 64],
            texture: [0; 28],
            cid: [0; 4],
            linear_a: [0; 16],
            linear_b: [0; 16],
        }
    }
    fn reset(&mut self) {
        *self = Self::new();
    }
    fn read(&self, address: u64) -> Option<u64> {
        tlb_slot(address).map(|slot| match slot {
            TlbSlot::FramebufferA(index) => self.framebuffer_a[index],
            TlbSlot::FramebufferB(index) => self.framebuffer_b[index],
            TlbSlot::FramebufferC(index) => self.framebuffer_c[index],
            TlbSlot::Texture(index) => self.texture[index],
            TlbSlot::Cid(index) => self.cid[index],
            TlbSlot::LinearA(index) => self.linear_a[index],
            TlbSlot::LinearB(index) => self.linear_b[index],
        })
    }
    fn write(&mut self, address: u64, value: u64) -> bool {
        let Some(slot) = tlb_slot(address) else {
            return false;
        };
        match slot {
            TlbSlot::FramebufferA(index) => self.framebuffer_a[index] = value,
            TlbSlot::FramebufferB(index) => self.framebuffer_b[index] = value,
            TlbSlot::FramebufferC(index) => self.framebuffer_c[index] = value,
            TlbSlot::Texture(index) => self.texture[index] = value,
            TlbSlot::Cid(index) => self.cid[index] = value,
            TlbSlot::LinearA(index) => self.linear_a[index] = value,
            TlbSlot::LinearB(index) => self.linear_b[index] = value,
        }
        true
    }
    fn linear_a_entries(&self) -> [u32; LINEAR_PAGE_COUNT] {
        Self::linear_entries(&self.linear_a)
    }
    fn linear_b_entries(&self) -> [u32; LINEAR_PAGE_COUNT] {
        Self::linear_entries(&self.linear_b)
    }
    fn framebuffer_entries(&self, buffer: u8) -> Option<FramebufferTlbSnapshot> {
        let slots = match buffer {
            0 => &self.framebuffer_a,
            1 => &self.framebuffer_b,
            2 => &self.framebuffer_c,
            _ => return None,
        };
        let mut entries = [0; FRAMEBUFFER_TLB_ENTRY_COUNT];
        for (slot_index, slot) in slots.iter().copied().enumerate() {
            entries[slot_index * 4] = (slot >> 48) as u16;
            entries[slot_index * 4 + 1] = (slot >> 32) as u16;
            entries[slot_index * 4 + 2] = (slot >> 16) as u16;
            entries[slot_index * 4 + 3] = slot as u16;
        }
        Some(FramebufferTlbSnapshot(entries))
    }
    fn linear_entries(slots: &[u64; 16]) -> [u32; LINEAR_PAGE_COUNT] {
        let mut entries = [0; LINEAR_PAGE_COUNT];
        let mut slot = 0;
        while slot < slots.len() {
            entries[slot * 2] = (slots[slot] >> 32) as u32;
            entries[slot * 2 + 1] = slots[slot] as u32;
            slot += 1;
        }
        entries
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TlbSlot {
    FramebufferA(usize),
    FramebufferB(usize),
    FramebufferC(usize),
    Texture(usize),
    Cid(usize),
    LinearA(usize),
    LinearB(usize),
}
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct FramebufferTlbSnapshot(
    #[serde(with = "crate::common::serde_array")] [u16; FRAMEBUFFER_TLB_ENTRY_COUNT],
);
impl FramebufferTlbSnapshot {
    fn entry(&self, index: usize) -> FramebufferTlbEntry {
        FramebufferTlbEntry(self.0[index])
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct FramebufferTlbEntry(u16);
impl FramebufferTlbEntry {
    const fn raw(self) -> u16 {
        self.0
    }
    const fn valid(self) -> bool {
        self.0 & FRAMEBUFFER_TLB_VALID != 0
    }
    const fn alias_address(self, tile_offset: u64) -> u64 {
        ((self.0 & FRAMEBUFFER_TLB_TILE_MASK) as u64) * FRAMEBUFFER_TILE_BYTES + tile_offset
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct PixelCandidateBatch {
    x: u16,
    y: u16,
    candidate_count: u16,
    enabled_count: u16,
    stipple_index: Option<u8>,
}
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct PixelPipelineJob {
    command: DecodedPixelCommand,
    entries: Box<FramebufferTlbSnapshot>,
    rasterizer: Rasterizer,
    stipple: Option<PixelStippleCursor>,
    pending_batch: Option<PixelCandidateBatch>,
}
impl PixelPipelineJob {
    fn complete(&self) -> bool {
        self.pending_batch.is_none() && self.rasterizer.complete()
    }
    fn endpoints(&self) -> (u16, u16, u16, u16) {
        self.command.endpoints()
    }
    fn advance_candidates(&mut self, candidates: u16) {
        self.rasterizer.advance(candidates);
        if let Some(stipple) = self.stipple.as_mut() {
            stipple.advance(candidates);
        }
    }
    fn complete_pending_batch(&mut self) -> Option<PixelCandidateBatch> {
        let batch = self.pending_batch.take()?;
        self.advance_candidates(batch.candidate_count);
        Some(batch)
    }
}
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct BlockedPixelCommand {
    command: DecodedPixelCommand,
    error: CrimeRenderError,
}
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
enum PixelExecution {
    Blocked(BlockedPixelCommand),
    Running(PixelPipelineJob),
}
impl PixelExecution {
    fn complete(&self) -> bool {
        match self {
            Self::Blocked(_) => false,
            Self::Running(job) => job.complete(),
        }
    }
    fn completion_notice(&self) -> RenderNotice {
        let Self::Running(job) = self else {
            unreachable!("blocked pixel command cannot complete")
        };
        let (x0, y0, x1, y1) = job.endpoints();
        RenderNotice::PixelCommandCompleted {
            primitive: job.command.primitive_raw,
            x0,
            y0,
            x1,
            y1,
        }
    }
    fn blocked_error(&self) -> Option<&CrimeRenderError> {
        match self {
            Self::Blocked(blocked) => Some(&blocked.error),
            Self::Running(_) => None,
        }
    }
    fn running_mut(&mut self) -> Option<&mut PixelPipelineJob> {
        match self {
            Self::Blocked(_) => None,
            Self::Running(job) => Some(job),
        }
    }
}
/// Rendering Engine execution failure.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum CrimeRenderError {
    /// A frozen pixel command contained one or more proven-invalid fields.
    InvalidPixelCommand {
        /// Canonical register whose START alias submitted the command.
        trigger_address: u64,
        /// Primitive register captured by the command snapshot.
        primitive: u32,
        /// Draw-mode register captured by the command snapshot.
        draw_mode: u32,
        /// Source BufMode register captured by the command snapshot.
        source_buffer_mode: u32,
        /// Destination BufMode register captured by the command snapshot.
        destination_buffer_mode: u32,
        /// Decoded set of the 24 defined DrawMode feature bits.
        feature_bits: u32,
        /// Complete stable list of invalid fields.
        violations: Vec<PixelCommandViolation>,
    },
    /// A pixel command is legal but requires unavailable evidence or behavior.
    UnsupportedPixelCommand {
        /// Canonical register whose START alias submitted the command.
        trigger_address: u64,
        /// Primitive register captured by the command snapshot.
        primitive: u32,
        /// Draw-mode register captured by the command snapshot.
        draw_mode: u32,
        /// Source BufMode register captured by the command snapshot.
        source_buffer_mode: u32,
        /// Destination BufMode register captured by the command snapshot.
        destination_buffer_mode: u32,
        /// Decoded set of the 24 defined DrawMode feature bits.
        feature_bits: u32,
        /// Complete stable list of missing capabilities.
        blockers: Vec<PixelCommandBlocker>,
    },
    /// The committed MTE state is outside the evidence-complete zero-clear subset.
    UnsupportedMteJob {
        /// MTE mode value.
        mode: u32,
        /// MTE byte mask.
        byte_mask: u32,
        /// MTE foreground value.
        foreground: u32,
    },
    /// The inclusive MTE destination endpoints are not valid for the selected buffer.
    InvalidMteRange {
        /// Inclusive start address.
        start: u32,
        /// Inclusive end address.
        end: u32,
    },
    /// A memory completion arrived while no Rendering Engine write was outstanding.
    UnexpectedMemoryCompletion,
    /// The memory target returned a payload incompatible with a Rendering Engine write.
    UnexpectedMemoryPayload,
    /// The memory domain failed to transport the Rendering Engine request.
    MemoryTransport(CrimeBusError),
}
impl fmt::Display for CrimeRenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPixelCommand {
                trigger_address,
                primitive,
                draw_mode,
                source_buffer_mode,
                destination_buffer_mode,
                feature_bits,
                violations,
            } => write!(
                f,
                "invalid CRIME pixel command triggered by {trigger_address:#010x}, primitive {primitive:#010x}, draw mode {draw_mode:#010x}, features {feature_bits:#08x}, source format {source_buffer_mode:#010x}, destination format {destination_buffer_mode:#010x}: {violations:?}"
            ),
            Self::UnsupportedPixelCommand {
                trigger_address,
                primitive,
                draw_mode,
                source_buffer_mode,
                destination_buffer_mode,
                feature_bits,
                blockers,
            } => write!(
                f,
                "unsupported CRIME pixel command triggered by {trigger_address:#010x}, primitive {primitive:#010x}, draw mode {draw_mode:#010x}, features {feature_bits:#08x}, source format {source_buffer_mode:#010x}, destination format {destination_buffer_mode:#010x}: {blockers:?}"
            ),
            Self::UnsupportedMteJob {
                mode,
                byte_mask,
                foreground,
            } => write!(
                f,
                "unsupported CRIME MTE job mode {mode:#010x}, byte mask {byte_mask:#010x}, foreground {foreground:#010x}"
            ),
            Self::InvalidMteRange { start, end } => {
                write!(f, "invalid CRIME MTE range {start:#010x}..={end:#010x}")
            }
            Self::UnexpectedMemoryCompletion => {
                f.write_str("unexpected CRIME Rendering Engine memory completion")
            }
            Self::UnexpectedMemoryPayload => f.write_str(
                "CRIME Rendering Engine memory completion returned an unexpected payload",
            ),
            Self::MemoryTransport(error) => {
                write!(
                    f,
                    "CRIME Rendering Engine memory transport failed: {error:?}"
                )
            }
        }
    }
}
impl std::error::Error for CrimeRenderError {}
/// Consumer that will resume when one RE memory request completes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(super) enum RenderMemoryDestination {
    Mte,
    Pixel,
}
/// One memory operation requested through the CRIME memory domain.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(super) struct RenderMemoryRequest {
    pub(super) virtual_address: u32,
    pub(super) raw_entry: u32,
    pub(super) valid: bool,
    pub(super) alias_address: u64,
    pub(super) physical_address: u64,
    pub(super) bank_select: CrimeMemoryBankSelect,
    pub(super) no_ecc: bool,
    pub(super) destination: RenderMemoryDestination,
    pub(super) transfer: CrimeTransfer,
}
/// A software-visible Rendering Engine transition used for tracing.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(super) enum RenderNotice {
    RegisterRetired(RenderRegisterWrite),
    JobCommitted {
        start: u32,
        end: u32,
    },
    PixelCommandDecoded {
        primitive: u32,
        draw_mode: u32,
        feature_bits: u32,
        violation_count: u16,
        blocker_count: u16,
    },
    PixelCommandCommitted {
        primitive: u32,
        x0: u16,
        y0: u16,
        x1: u16,
        y1: u16,
    },
    RasterBatch {
        x: u16,
        y: u16,
        candidates: u16,
        enabled: u16,
    },
    FramebufferWordLayout {
        logical_lane: u8,
        physical_lane: u8,
        bytes_per_pixel: u8,
    },
    StippleMask {
        pattern: u32,
        index: u8,
        candidates: u16,
        enabled_mask: u32,
    },
    MemoryChunk {
        destination: RenderMemoryDestination,
        virtual_address: u32,
        physical_address: u64,
        length: u16,
    },
    MemoryCompleted {
        destination: RenderMemoryDestination,
        virtual_address: u32,
        physical_address: u64,
        length: u16,
    },
    TlbTranslation {
        virtual_address: u32,
        raw_entry: u32,
        valid: bool,
        alias_address: u64,
        physical_address: u64,
    },
    JobCompleted {
        start: u32,
        end: u32,
    },
    PixelCommandCompleted {
        primitive: u32,
        x0: u16,
        y0: u16,
        x1: u16,
        y1: u16,
    },
}
/// One PIU interrupt-source transition generated by the RE.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(super) struct RenderInterruptEffect {
    pub(super) mask: u32,
    pub(super) asserted: bool,
}
/// Effects produced by one RE state transition.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(super) struct RenderProgress {
    pub(super) schedule_step: bool,
    pub(super) memory_request: Option<RenderMemoryRequest>,
    pub(super) interrupts: Vec<RenderInterruptEffect>,
    pub(super) notices: Vec<RenderNotice>,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct MteRegisters {
    mode: u32,
    byte_mask: u32,
    stipple_mask: u32,
    foreground: u32,
    source_start: u32,
    source_end: u32,
    destination_start: u32,
    destination_end: u32,
    source_y_step: u32,
    destination_y_step: u32,
}
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct MteJob {
    start: u32,
    end: u32,
    destination: MteDestination,
    no_ecc: bool,
}
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
enum MteDestination {
    Linear {
        next: u64,
        end: u64,
        entries: [u32; LINEAR_PAGE_COUNT],
    },
    Framebuffer {
        entries: Box<FramebufferTlbSnapshot>,
        bytes_per_pixel: u8,
        x_start: u16,
        x_end: u16,
        x: u16,
        y_end: u16,
        y: u16,
    },
}
impl MteJob {
    fn complete(&self) -> bool {
        match &self.destination {
            MteDestination::Linear { next, end, .. } => next > end,
            MteDestination::Framebuffer { y, y_end, .. } => y > y_end,
        }
    }
    fn advance(&mut self, length: u16) {
        match &mut self.destination {
            MteDestination::Linear { next, .. } => *next += u64::from(length),
            MteDestination::Framebuffer {
                bytes_per_pixel,
                x_start,
                x_end,
                x,
                y,
                ..
            } => {
                let pixels = length / u16::from(*bytes_per_pixel);
                let next = u32::from(*x) + u32::from(pixels);
                if next > u32::from(*x_end) {
                    *x = *x_start;
                    *y = y.saturating_add(1);
                } else {
                    *x = next as u16;
                }
            }
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct PendingRenderMemory {
    destination: RenderMemoryDestination,
    virtual_address: u32,
    physical_address: u64,
    length: u16,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct MemoryRequestUnit {
    pending: Option<PendingRenderMemory>,
}
impl MemoryRequestUnit {
    const fn new() -> Self {
        Self { pending: None }
    }
    const fn busy(&self) -> bool {
        self.pending.is_some()
    }
    fn issue(&mut self, request: &RenderMemoryRequest) {
        debug_assert!(self.pending.is_none());
        self.pending = Some(PendingRenderMemory {
            destination: request.destination,
            virtual_address: request.virtual_address,
            physical_address: request.physical_address,
            length: request.transfer.length() as u16,
        });
    }
    fn complete(&mut self) -> Option<PendingRenderMemory> {
        self.pending.take()
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct LinearTlbEntry(u32);
impl LinearTlbEntry {
    const fn valid(self) -> bool {
        self.0 & 0x8000_0000 != 0
    }
    const fn alias_address(self, page_offset: u64) -> u64 {
        ((self.0 & LINEAR_PAGE_MASK) as u64) * LINEAR_PAGE_SIZE + page_offset
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct RenderConditions {
    empty: bool,
    full: bool,
    idle: bool,
}
/// CRIME Rendering Engine front-end state.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CrimeRender {
    interface: HostInterface,
    tlbs: RenderTlbs,
    pixel: PixelRegisters,
    mte: MteRegisters,
    active_pixel_command: Option<PixelExecution>,
    active_job: Option<MteJob>,
    memory_request_unit: MemoryRequestUnit,
    step_scheduled: bool,
    conditions: RenderConditions,
    epoch: u64,
}
impl CrimeRender {
    /// Creates reset Rendering Engine state.
    pub const fn new() -> Self {
        Self {
            interface: HostInterface::new(),
            tlbs: RenderTlbs::new(),
            pixel: PixelRegisters::new(),
            mte: MteRegisters {
                mode: 0,
                byte_mask: 0,
                stipple_mask: 0,
                foreground: 0,
                source_start: 0,
                source_end: 0,
                destination_start: 0,
                destination_end: 0,
                source_y_step: 0,
                destination_y_step: 0,
            },
            active_pixel_command: None,
            active_job: None,
            memory_request_unit: MemoryRequestUnit::new(),
            step_scheduled: false,
            conditions: RenderConditions {
                empty: true,
                full: false,
                idle: true,
            },
            epoch: 0,
        }
    }
    /// Resets the front end and invalidates old render events.
    pub(super) fn reset(&mut self) -> Vec<RenderInterruptEffect> {
        self.interface = HostInterface::new();
        self.tlbs.reset();
        self.pixel.reset();
        self.mte = MteRegisters::default();
        self.active_pixel_command = None;
        self.active_job = None;
        self.memory_request_unit = MemoryRequestUnit::new();
        self.step_scheduled = false;
        self.conditions = RenderConditions {
            empty: true,
            full: false,
            idle: true,
        };
        self.epoch = self.epoch.wrapping_add(1);
        vec![
            RenderInterruptEffect {
                mask: registers::INTERRUPT_RE_EMPTY_LEVEL,
                asserted: true,
            },
            RenderInterruptEffect {
                mask: registers::INTERRUPT_RE_FULL_LEVEL,
                asserted: false,
            },
            RenderInterruptEffect {
                mask: registers::INTERRUPT_RE_IDLE_LEVEL,
                asserted: true,
            },
        ]
    }
    /// Returns the active Rendering Engine epoch.
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }
    /// Returns the number of host writes waiting in the 64-entry interface buffer.
    pub fn interface_level(&self) -> usize {
        self.interface.level()
    }
    /// Returns whether another host register write can be accepted.
    pub(super) fn has_interface_space(&self) -> bool {
        self.interface.can_accept()
    }
    /// Reads a software-visible Rendering Engine register.
    pub fn read(&self, address: u64, size: u8) -> Result<u64, RenderAccessError> {
        let access = register_access(address, size, RegisterDirection::Read)?;
        if !access.readable {
            return Err(RenderAccessError::Unsupported);
        }
        if address == STATUS_BASE {
            return Ok(u64::from(self.status()));
        }
        if address == INTERFACE_CONTROL {
            return Ok(u64::from(self.interface.control));
        }
        if let Some(value) = self.interface.read_ram(address) {
            return Ok(value);
        }
        if let Some(value) = self.tlbs.read(address) {
            return Ok(value);
        }
        if (PIXEL_PIPE_BASE..PIXEL_PIPE_BASE + 0x200).contains(&address) {
            return Ok(self.pixel.read(address, size));
        }
        if (MTE_BASE..MTE_BASE + 0x80).contains(&address) {
            return Ok(u64::from(self.read_mte(address)));
        }
        Err(RenderAccessError::Unsupported)
    }
    /// Queues one software-visible Rendering Engine register write.
    pub(super) fn write(
        &mut self,
        address: u64,
        size: u8,
        value: u64,
    ) -> Result<RenderProgress, RenderWriteError> {
        let (address, commit) = canonical_write_address(address);
        let access = register_access(address, size, RegisterDirection::Write)
            .map_err(RenderWriteError::Access)?;
        if !access.writable || commit && !access.start_eligible {
            return Err(RenderWriteError::Access(RenderAccessError::Unsupported));
        }
        let previous = self.conditions;
        if !access.buffered {
            if self.interface.write_ram(address, value) {
                return Ok(RenderProgress::default());
            }
            match address {
                INTERFACE_CONTROL => self.interface.set_control(value as u32),
                INTERFACE_RESET => {
                    self.interface.reset();
                    self.step_scheduled = false;
                    self.epoch = self.epoch.wrapping_add(1);
                }
                SET_START_POINTER => self.interface.set_start_pointer(),
                _ => return Err(RenderWriteError::Access(RenderAccessError::Unsupported)),
            }
            let schedule_step = self.ensure_step_scheduled();
            return Ok(RenderProgress {
                schedule_step,
                interrupts: self.update_conditions(previous),
                ..RenderProgress::default()
            });
        }
        if !self.has_interface_space() {
            return Err(RenderWriteError::InterfaceFull);
        }
        self.interface.accept(RenderRegisterWrite {
            address,
            value,
            size,
            commit,
        });
        let schedule_step = self.ensure_step_scheduled();
        let interrupts = self.update_conditions(previous);
        Ok(RenderProgress {
            schedule_step,
            interrupts,
            ..RenderProgress::default()
        })
    }
    /// Advances one bounded RE state-machine step.
    pub(super) fn step(&mut self) -> Result<RenderProgress, CrimeRenderError> {
        let previous = self.conditions;
        self.step_scheduled = false;
        self.interface.advance_stall();
        let mut progress = RenderProgress::default();
        if self.active_pixel_command.is_some() {
            if self.memory_request_unit.busy() {
                progress.schedule_step = self.ensure_step_scheduled();
                progress.interrupts = self.update_conditions(previous);
                return Ok(progress);
            }
            if let Some(error) = self
                .active_pixel_command
                .as_ref()
                .and_then(PixelExecution::blocked_error)
            {
                return Err(error.clone());
            }
            if self
                .active_pixel_command
                .as_ref()
                .is_some_and(PixelExecution::complete)
            {
                let command = self
                    .active_pixel_command
                    .take()
                    .expect("complete pixel command exists");
                progress.notices.push(command.completion_notice());
                progress.schedule_step = self.ensure_step_scheduled();
            } else {
                if let Some(memory_request) = self.prepare_pixel_batch(&mut progress.notices) {
                    append_memory_notices(&mut progress.notices, &memory_request);
                    self.memory_request_unit.issue(&memory_request);
                    progress.memory_request = Some(memory_request);
                }
                progress.schedule_step = self.ensure_step_scheduled();
            }
            progress.interrupts = self.update_conditions(previous);
            return Ok(progress);
        }
        if self.active_job.is_some() {
            if self.memory_request_unit.busy() {
                progress.schedule_step = self.ensure_step_scheduled();
                progress.interrupts = self.update_conditions(previous);
                return Ok(progress);
            }
            if self.active_job.as_ref().is_some_and(MteJob::complete) {
                let job = self.active_job.take().expect("active job exists");
                progress.notices.push(RenderNotice::JobCompleted {
                    start: job.start,
                    end: job.end,
                });
                progress.schedule_step = self.ensure_step_scheduled();
            } else {
                let memory_request = self.prepare_mte_memory_request()?;
                append_memory_notices(&mut progress.notices, &memory_request);
                self.memory_request_unit.issue(&memory_request);
                progress.memory_request = Some(memory_request);
                progress.schedule_step = self.ensure_step_scheduled();
            }
            progress.interrupts = self.update_conditions(previous);
            return Ok(progress);
        }
        let Some(write) = self.interface.retire() else {
            progress.schedule_step = self.ensure_step_scheduled();
            progress.interrupts = self.update_conditions(previous);
            return Ok(progress);
        };
        self.apply_register_write(write);
        progress.notices.push(RenderNotice::RegisterRetired(write));
        if write.commit {
            if (MTE_BASE..=MTE_BASE + 0x78).contains(&write.address) {
                if matches!(write.address - MTE_BASE, 0x70 | 0x78) {
                    progress.schedule_step = self.ensure_step_scheduled();
                    progress.interrupts = self.update_conditions(previous);
                    return Ok(progress);
                }
                let job = self.snapshot_zero_clear_job()?;
                progress.notices.push(RenderNotice::JobCommitted {
                    start: job.start,
                    end: job.end,
                });
                self.active_job = Some(job);
                let memory_request = self.prepare_mte_memory_request()?;
                append_memory_notices(&mut progress.notices, &memory_request);
                self.memory_request_unit.issue(&memory_request);
                progress.memory_request = Some(memory_request);
            } else {
                let command = self.pixel.command_snapshot(write.address);
                match self.snapshot_pixel_execution(command) {
                    Ok(job) => {
                        let (x0, y0, x1, y1) = job.endpoints();
                        progress.notices.push(RenderNotice::PixelCommandDecoded {
                            primitive: job.command.primitive_raw,
                            draw_mode: job.command.snapshot.draw_mode(),
                            feature_bits: job.command.features.raw(),
                            violation_count: 0,
                            blocker_count: 0,
                        });
                        progress.notices.push(RenderNotice::PixelCommandCommitted {
                            primitive: job.command.primitive_raw,
                            x0,
                            y0,
                            x1,
                            y1,
                        });
                        self.active_pixel_command = Some(PixelExecution::Running(job));
                        if let Some(memory_request) =
                            self.prepare_pixel_batch(&mut progress.notices)
                        {
                            append_memory_notices(&mut progress.notices, &memory_request);
                            self.memory_request_unit.issue(&memory_request);
                            progress.memory_request = Some(memory_request);
                        } else {
                            progress.schedule_step = self.ensure_step_scheduled();
                        }
                    }
                    Err(blocked) => {
                        let blocked = *blocked;
                        let error = blocked.error.clone();
                        self.active_pixel_command = Some(PixelExecution::Blocked(blocked));
                        return Err(error);
                    }
                }
            }
        } else {
            progress.schedule_step = self.ensure_step_scheduled();
        }
        progress.interrupts = self.update_conditions(previous);
        Ok(progress)
    }
    /// Completes the outstanding MTE memory write.
    pub(super) fn complete_memory(
        &mut self,
        result: Result<CrimeMemoryOutcome, CrimeBusError>,
    ) -> Result<RenderProgress, CrimeRenderError> {
        let previous = self.conditions;
        let Some(pending) = self.memory_request_unit.complete() else {
            return Err(CrimeRenderError::UnexpectedMemoryCompletion);
        };
        let outcome = result.map_err(CrimeRenderError::MemoryTransport)?;
        if !matches!(outcome.payload, CrimeCompletionPayload::WriteComplete) {
            return Err(CrimeRenderError::UnexpectedMemoryPayload);
        }
        match pending.destination {
            RenderMemoryDestination::Mte => {
                let Some(job) = self.active_job.as_mut() else {
                    return Err(CrimeRenderError::UnexpectedMemoryCompletion);
                };
                job.advance(pending.length);
            }
            RenderMemoryDestination::Pixel => {
                let Some(command) = self.active_pixel_command.as_mut() else {
                    return Err(CrimeRenderError::UnexpectedMemoryCompletion);
                };
                let Some(job) = command.running_mut() else {
                    return Err(CrimeRenderError::UnexpectedMemoryCompletion);
                };
                if job.complete_pending_batch().is_none() {
                    return Err(CrimeRenderError::UnexpectedMemoryCompletion);
                }
            }
        }
        let schedule_step = self.ensure_step_scheduled();
        Ok(RenderProgress {
            schedule_step,
            interrupts: self.update_conditions(previous),
            notices: vec![RenderNotice::MemoryCompleted {
                destination: pending.destination,
                virtual_address: pending.virtual_address,
                physical_address: pending.physical_address,
                length: pending.length,
            }],
            ..RenderProgress::default()
        })
    }
    fn status(&self) -> u32 {
        let setup_idle = self.interface.level == 0;
        let pixel_idle = self.active_pixel_command.is_none();
        let mte_idle = self.active_job.is_none();
        let idle = setup_idle && pixel_idle && mte_idle;
        (if idle { STATUS_IDLE } else { 0 })
            | (if setup_idle { STATUS_SETUP_IDLE } else { 0 })
            | (if pixel_idle {
                STATUS_PIXEL_PIPE_IDLE
            } else {
                0
            })
            | (if mte_idle { STATUS_MTE_IDLE } else { 0 })
            | u32::from(self.interface.level) << STATUS_LEVEL_SHIFT
            | u32::from(self.interface.read_pointer) << STATUS_READ_POINTER_SHIFT
            | u32::from(self.interface.write_pointer) << STATUS_WRITE_POINTER_SHIFT
            | u32::from(self.interface.start_pointer) << STATUS_START_POINTER_SHIFT
    }
    fn ensure_step_scheduled(&mut self) -> bool {
        let work_ready = self.interface.stall_cycles != 0
            || (!self.memory_request_unit.busy()
                && (matches!(self.active_pixel_command, Some(PixelExecution::Running(_)))
                    || self.active_job.is_some()))
            || (self.active_pixel_command.is_none()
                && self.active_job.is_none()
                && self.interface.level != 0);
        if !work_ready || self.step_scheduled {
            return false;
        }
        self.step_scheduled = true;
        true
    }
    fn update_conditions(&mut self, previous: RenderConditions) -> Vec<RenderInterruptEffect> {
        let current = RenderConditions {
            empty: self.interface.empty_condition(),
            full: self.interface.full_condition(),
            idle: self.interface.level == 0
                && self.active_pixel_command.is_none()
                && self.active_job.is_none(),
        };
        self.conditions = current;
        let mut effects = Vec::new();
        append_condition_effects(
            &mut effects,
            previous.empty,
            current.empty,
            registers::INTERRUPT_RE_EMPTY_EDGE,
            registers::INTERRUPT_RE_EMPTY_LEVEL,
        );
        append_condition_effects(
            &mut effects,
            previous.full,
            current.full,
            registers::INTERRUPT_RE_FULL_EDGE,
            registers::INTERRUPT_RE_FULL_LEVEL,
        );
        append_condition_effects(
            &mut effects,
            previous.idle,
            current.idle,
            registers::INTERRUPT_RE_IDLE_EDGE,
            registers::INTERRUPT_RE_IDLE_LEVEL,
        );
        effects
    }
    fn apply_register_write(&mut self, write: RenderRegisterWrite) {
        if self.tlbs.write(write.address, write.value) {
            return;
        }
        if (PIXEL_PIPE_BASE..PIXEL_PIPE_BASE + 0x200).contains(&write.address) {
            self.pixel.write(write.address, write.size, write.value);
            return;
        }
        let value = write.value as u32;
        if (MTE_BASE..=MTE_BASE + 0x78).contains(&write.address) {
            match write.address - MTE_BASE {
                0x00 => self.mte.mode = value,
                0x08 => self.mte.byte_mask = value,
                0x10 => self.mte.stipple_mask = value,
                0x18 => self.mte.foreground = value,
                0x20 => self.mte.source_start = value,
                0x28 => self.mte.source_end = value,
                0x30 => self.mte.destination_start = value,
                0x38 => self.mte.destination_end = value,
                0x40 => self.mte.source_y_step = value,
                0x48 => self.mte.destination_y_step = value,
                _ => {}
            }
        }
    }
    const fn read_mte(&self, address: u64) -> u32 {
        match address - MTE_BASE {
            0x00 => self.mte.mode,
            0x08 => self.mte.byte_mask,
            0x18 => self.mte.foreground,
            0x20 => self.mte.source_start,
            0x28 => self.mte.source_end,
            0x40 => self.mte.source_y_step,
            0x48 => self.mte.destination_y_step,
            _ => 0,
        }
    }
    fn snapshot_pixel_execution(
        &self,
        command: PixelCommandSnapshot,
    ) -> Result<PixelPipelineJob, Box<BlockedPixelCommand>> {
        let validation = PixelCommandValidation::decode(command);
        let error = if validation.violations.is_empty() {
            if validation.blockers.is_empty() {
                None
            } else {
                Some(CrimeRenderError::UnsupportedPixelCommand {
                    trigger_address: validation.decoded.snapshot.trigger_address,
                    primitive: validation.decoded.primitive_raw,
                    draw_mode: validation.decoded.snapshot.draw_mode(),
                    source_buffer_mode: validation.decoded.source.raw,
                    destination_buffer_mode: validation.decoded.destination.raw,
                    feature_bits: validation.decoded.features.raw(),
                    blockers: validation.blockers,
                })
            }
        } else {
            Some(CrimeRenderError::InvalidPixelCommand {
                trigger_address: validation.decoded.snapshot.trigger_address,
                primitive: validation.decoded.primitive_raw,
                draw_mode: validation.decoded.snapshot.draw_mode(),
                source_buffer_mode: validation.decoded.source.raw,
                destination_buffer_mode: validation.decoded.destination.raw,
                feature_bits: validation.decoded.features.raw(),
                violations: validation.violations,
            })
        };
        if let Some(error) = error {
            return Err(Box::new(BlockedPixelCommand {
                command: validation.decoded,
                error,
            }));
        }
        let command = validation.decoded;
        let rasterizer = match command.primitive_kind {
            PixelPrimitiveKind::Line => {
                let direction = if command.y0 == command.y1 {
                    AxisLineDirection::Horizontal
                } else {
                    AxisLineDirection::Vertical
                };
                Rasterizer::AxisLine(AxisLineRasterizer {
                    direction,
                    x: command.x0,
                    y: command.y0,
                    end_x: command.x1,
                    end_y: command.y1,
                })
            }
            PixelPrimitiveKind::Rectangle => {
                Rasterizer::InclusiveRectangle(InclusiveRectangleRasterizer {
                    x_start: command.x0,
                    x: command.x0,
                    y: command.y0,
                    end_x: command.x1,
                    end_y: command.y1,
                    row_direction: if command.edge_type == 0 {
                        RectangleRowDirection::Descending
                    } else {
                        RectangleRowDirection::Ascending
                    },
                    finished: false,
                })
            }
            _ => unreachable!("validated command has an implemented rasterizer"),
        };
        let selector = command
            .destination
            .kind
            .framebuffer_selector()
            .expect("validated command selects a framebuffer");
        let stipple = command
            .line_stipple_enabled()
            .then(|| PixelStippleCursor::new(command.stipple_pattern, command.stipple_mode));
        Ok(PixelPipelineJob {
            entries: Box::new(
                self.tlbs
                    .framebuffer_entries(selector)
                    .expect("validated framebuffer selector"),
            ),
            command,
            rasterizer,
            stipple,
            pending_batch: None,
        })
    }
    fn prepare_pixel_batch(
        &mut self,
        notices: &mut Vec<RenderNotice>,
    ) -> Option<RenderMemoryRequest> {
        let Some(PixelExecution::Running(job)) = self.active_pixel_command.as_mut() else {
            unreachable!("pixel batch requires an active running command")
        };
        debug_assert!(job.pending_batch.is_none());
        let position = job.rasterizer.position();
        let bytes_per_pixel = job
            .command
            .destination
            .format
            .bytes_per_pixel()
            .expect("validated command has a sized pixel format");
        let tile_width = (FRAMEBUFFER_TILE_ROW_BYTES / u64::from(bytes_per_pixel)) as u16;
        let tile_x = usize::from(position.x / tile_width);
        let tile_y = usize::from(position.y / FRAMEBUFFER_TILE_HEIGHT);
        let entry = job
            .entries
            .entry(tile_y * FRAMEBUFFER_TILES_PER_ROW + tile_x);
        let x_in_tile = position.x % tile_width;
        let y_in_tile = position.y % FRAMEBUFFER_TILE_HEIGHT;
        let pixel_offset = u64::from(y_in_tile) * FRAMEBUFFER_TILE_ROW_BYTES
            + u64::from(x_in_tile) * u64::from(bytes_per_pixel);
        let pixel_alias = entry.alias_address(pixel_offset);
        let word_alias = pixel_alias & !(RENDER_MEMORY_WORD_BYTES as u64 - 1);
        let first_logical_lane = (pixel_alias - word_alias) as usize;
        let candidate_count = if job.rasterizer.contiguous() {
            usize::from(job.rasterizer.remaining_in_row())
                .min((RENDER_MEMORY_WORD_BYTES - first_logical_lane) / usize::from(bytes_per_pixel))
                as u16
        } else {
            1
        };
        let pixel_bytes = job.command.pixel_bytes();
        let mut data = vec![0; RENDER_MEMORY_WORD_BYTES];
        let mut byte_enable = vec![false; RENDER_MEMORY_WORD_BYTES];
        let first_physical_lane =
            framebuffer::physical_pixel_lane(first_logical_lane, usize::from(bytes_per_pixel))
                .expect("validated pixel batch starts inside one framebuffer word");
        notices.push(RenderNotice::FramebufferWordLayout {
            logical_lane: first_logical_lane as u8,
            physical_lane: first_physical_lane as u8,
            bytes_per_pixel,
        });
        let mut enabled_count = 0_u16;
        let mut enabled_mask = 0_u32;
        for candidate in 0..candidate_count {
            let enabled = job.stipple.is_none_or(|stipple| stipple.permits(candidate));
            if !enabled {
                continue;
            }
            enabled_count += 1;
            enabled_mask |= 1_u32 << (31 - candidate);
            let bytes = usize::from(bytes_per_pixel);
            let logical_lane = first_logical_lane + usize::from(candidate) * bytes;
            let lane = framebuffer::physical_pixel_lane(logical_lane, bytes)
                .expect("validated pixel batch remains inside one framebuffer word");
            data[lane..lane + bytes].copy_from_slice(&pixel_bytes[..bytes]);
            byte_enable[lane..lane + bytes].fill(true);
        }
        let batch = PixelCandidateBatch {
            x: position.x,
            y: position.y,
            candidate_count,
            enabled_count,
            stipple_index: job.stipple.map(PixelStippleCursor::index),
        };
        notices.push(RenderNotice::RasterBatch {
            x: position.x,
            y: position.y,
            candidates: candidate_count,
            enabled: enabled_count,
        });
        if let Some(stipple) = job.stipple {
            notices.push(RenderNotice::StippleMask {
                pattern: job.command.stipple_pattern,
                index: stipple.index(),
                candidates: candidate_count,
                enabled_mask,
            });
        }
        if enabled_count == 0 {
            job.advance_candidates(candidate_count);
            return None;
        }
        job.pending_batch = Some(batch);
        let physical_address = super::normalize_render_memory_alias(word_alias);
        let valid = entry.valid();
        Some(RenderMemoryRequest {
            virtual_address: u32::from(position.y) << 16 | u32::from(position.x),
            raw_entry: u32::from(entry.raw()),
            valid,
            alias_address: word_alias,
            physical_address,
            bank_select: if valid {
                CrimeMemoryBankSelect::Decode
            } else {
                CrimeMemoryBankSelect::Inhibited {
                    reason: CrimeMemoryInhibitReason::InvalidRenderTlb,
                }
            },
            no_ecc: false,
            destination: RenderMemoryDestination::Pixel,
            transfer: CrimeTransfer::write(data.into(), byte_enable.into()),
        })
    }
    fn snapshot_zero_clear_job(&self) -> Result<MteJob, CrimeRenderError> {
        let mode = self.mte.mode;
        let depth = ((mode >> 8) & 3) as u8;
        let source = ((mode >> 5) & 7) as u8;
        let destination = ((mode >> 2) & 7) as u8;
        let unsupported_mode = mode & !0x0000_0fff != 0
            || mode & (1 << 11) != 0
            || mode & (1 << 10) != 0
            || depth == 3
            || source != 0
            || !matches!(destination, 0..=2 | 4..=5)
            || mode & (1 << 1) != 0;
        if unsupported_mode || self.mte.byte_mask != u32::MAX || self.mte.foreground != 0 {
            return Err(CrimeRenderError::UnsupportedMteJob {
                mode,
                byte_mask: self.mte.byte_mask,
                foreground: self.mte.foreground,
            });
        }
        let start = self.mte.destination_start;
        let end = self.mte.destination_end;
        let no_ecc = mode & 1 == 0;
        let destination = match destination {
            4 | 5 => {
                if end < start {
                    return Err(CrimeRenderError::InvalidMteRange { start, end });
                }
                let entries = if destination == 4 {
                    self.tlbs.linear_a_entries()
                } else {
                    self.tlbs.linear_b_entries()
                };
                MteDestination::Linear {
                    next: u64::from(start),
                    end: u64::from(end),
                    entries,
                }
            }
            framebuffer => {
                let bytes_per_pixel = 1_u8 << depth;
                let x_start = start as u16;
                let x_end = end as u16;
                let y_byte_start = (start >> 16) as u16;
                let y_byte_end = (end >> 16) as u16;
                let bytes = u16::from(bytes_per_pixel);
                let y_start = y_byte_start / bytes;
                let y_end = y_byte_end / bytes;
                let tile_width = (FRAMEBUFFER_TILE_ROW_BYTES / u64::from(bytes_per_pixel)) as u16;
                let width = u32::from(tile_width) * FRAMEBUFFER_TILES_PER_ROW as u32;
                let height = u32::from(FRAMEBUFFER_TILE_HEIGHT)
                    * (FRAMEBUFFER_TLB_ENTRY_COUNT / FRAMEBUFFER_TILES_PER_ROW) as u32;
                let valid = x_end >= x_start
                    && y_byte_start.is_multiple_of(bytes)
                    && y_byte_end % bytes == bytes - 1
                    && y_end >= y_start
                    && u32::from(x_end) < width
                    && u32::from(y_end) < height;
                if !valid {
                    return Err(CrimeRenderError::InvalidMteRange { start, end });
                }
                MteDestination::Framebuffer {
                    entries: Box::new(
                        self.tlbs
                            .framebuffer_entries(framebuffer)
                            .expect("validated framebuffer selector"),
                    ),
                    bytes_per_pixel,
                    x_start,
                    x_end,
                    x: x_start,
                    y_end,
                    y: y_start,
                }
            }
        };
        Ok(MteJob {
            start,
            end,
            destination,
            no_ecc,
        })
    }
    fn prepare_mte_memory_request(&self) -> Result<RenderMemoryRequest, CrimeRenderError> {
        let job = self.active_job.as_ref().expect("active MTE job exists");
        let (virtual_address, raw_entry, valid, alias_address, length) = match &job.destination {
            MteDestination::Linear { next, end, entries } => {
                let virtual_address = *next as u32;
                let page_index = ((*next >> 12) & 0x1f) as usize;
                let entry = LinearTlbEntry(entries[page_index]);
                let in_page = *next & (LINEAR_PAGE_SIZE - 1);
                let remaining = *end - *next + 1;
                let length = remaining
                    .min(LINEAR_PAGE_SIZE - in_page)
                    .min(MAX_MEMORY_CHUNK_BYTES as u64) as usize;
                (
                    virtual_address,
                    entry.0,
                    entry.valid(),
                    entry.alias_address(in_page),
                    length,
                )
            }
            MteDestination::Framebuffer {
                entries,
                bytes_per_pixel,
                x_end,
                x,
                y,
                ..
            } => {
                let bytes = u16::from(*bytes_per_pixel);
                let tile_width = (FRAMEBUFFER_TILE_ROW_BYTES / u64::from(*bytes_per_pixel)) as u16;
                let tile_x = usize::from(*x / tile_width);
                let tile_y = usize::from(*y / FRAMEBUFFER_TILE_HEIGHT);
                let entry = entries.entry(tile_y * FRAMEBUFFER_TILES_PER_ROW + tile_x);
                let x_in_tile = *x % tile_width;
                let y_in_tile = *y % FRAMEBUFFER_TILE_HEIGHT;
                let tile_offset = u64::from(y_in_tile) * FRAMEBUFFER_TILE_ROW_BYTES
                    + u64::from(x_in_tile) * u64::from(*bytes_per_pixel);
                let remaining_pixels = u32::from(*x_end) - u32::from(*x) + 1;
                let tile_pixels = u32::from(tile_width - x_in_tile);
                let length = (remaining_pixels.min(tile_pixels) * u32::from(bytes)) as usize;
                let virtual_address = (u32::from(*y) * u32::from(bytes)) << 16 | u32::from(*x);
                (
                    virtual_address,
                    u32::from(entry.raw()),
                    entry.valid(),
                    entry.alias_address(tile_offset),
                    length,
                )
            }
        };
        let physical_address = super::normalize_render_memory_alias(alias_address);
        let bank_select = if valid {
            CrimeMemoryBankSelect::Decode
        } else {
            CrimeMemoryBankSelect::Inhibited {
                reason: CrimeMemoryInhibitReason::InvalidRenderTlb,
            }
        };
        let no_ecc = job.no_ecc;
        let request = RenderMemoryRequest {
            virtual_address,
            raw_entry,
            valid,
            alias_address,
            physical_address,
            bank_select,
            no_ecc,
            destination: RenderMemoryDestination::Mte,
            transfer: CrimeTransfer::write(
                CrimeData::zeroed(length),
                CrimeByteEnable::enabled(length),
            ),
        };
        Ok(request)
    }
}
fn append_memory_notices(notices: &mut Vec<RenderNotice>, request: &RenderMemoryRequest) {
    notices.push(RenderNotice::TlbTranslation {
        virtual_address: request.virtual_address,
        raw_entry: request.raw_entry,
        valid: request.valid,
        alias_address: request.alias_address,
        physical_address: request.physical_address,
    });
    notices.push(RenderNotice::MemoryChunk {
        destination: request.destination,
        virtual_address: request.virtual_address,
        physical_address: request.physical_address,
        length: request.transfer.length() as u16,
    });
}
impl Default for CrimeRender {
    fn default() -> Self {
        Self::new()
    }
}
/// Rendering Engine register-access classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum RenderAccessError {
    /// The address is defined, but the width or alignment is invalid.
    Access,
    /// The address is reserved, unmapped, or not readable in this direction.
    Unsupported,
}
/// Rendering Engine host-write failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum RenderWriteError {
    /// The register access was rejected before entering the host interface.
    Access(RenderAccessError),
    /// The 64-entry host interface buffer or its programmed stall is blocking writes.
    InterfaceFull,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct RegisterAccess {
    readable: bool,
    writable: bool,
    buffered: bool,
    start_eligible: bool,
    read_widths: u8,
    write_widths: u8,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RegisterDirection {
    Read,
    Write,
}
const WIDTH_32: u8 = 1;
const WIDTH_64: u8 = 2;
const DIRECT_READ_WRITE_64: RegisterAccess = RegisterAccess {
    readable: true,
    writable: true,
    buffered: false,
    start_eligible: false,
    read_widths: WIDTH_64,
    write_widths: WIDTH_64,
};
const DIRECT_READ_WRITE_32: RegisterAccess = RegisterAccess {
    readable: true,
    writable: true,
    buffered: false,
    start_eligible: false,
    read_widths: WIDTH_32,
    write_widths: WIDTH_32,
};
const DIRECT_WRITE_ONLY_32: RegisterAccess = RegisterAccess {
    readable: false,
    writable: true,
    buffered: false,
    start_eligible: false,
    read_widths: 0,
    write_widths: WIDTH_32,
};
const DIRECT_READ_ONLY_32: RegisterAccess = RegisterAccess {
    readable: true,
    writable: false,
    buffered: false,
    start_eligible: false,
    read_widths: WIDTH_32,
    write_widths: 0,
};
const BUFFERED_READ_WRITE_64: RegisterAccess = RegisterAccess {
    readable: true,
    writable: true,
    buffered: true,
    start_eligible: false,
    read_widths: WIDTH_64,
    write_widths: WIDTH_64,
};
const BUFFERED_READ_WRITE_32_START: RegisterAccess = RegisterAccess {
    readable: true,
    writable: true,
    buffered: true,
    start_eligible: true,
    read_widths: WIDTH_32,
    write_widths: WIDTH_32,
};
const BUFFERED_READ_WRITE_64_START: RegisterAccess = RegisterAccess {
    readable: true,
    writable: true,
    buffered: true,
    start_eligible: true,
    read_widths: WIDTH_64,
    write_widths: WIDTH_64,
};
const BUFFERED_READ_WRITE_WINDOW_OFFSET: RegisterAccess = RegisterAccess {
    readable: true,
    writable: true,
    buffered: true,
    start_eligible: true,
    read_widths: WIDTH_32,
    write_widths: WIDTH_32 | WIDTH_64,
};
const BUFFERED_WRITE_ONLY_32_START: RegisterAccess = RegisterAccess {
    readable: false,
    writable: true,
    buffered: true,
    start_eligible: true,
    read_widths: 0,
    write_widths: WIDTH_32,
};
const BUFFERED_WRITE_ONLY_64_START: RegisterAccess = RegisterAccess {
    readable: false,
    writable: true,
    buffered: true,
    start_eligible: true,
    read_widths: 0,
    write_widths: WIDTH_64,
};
const BUFFERED_WRITE_ONLY_32_OR_64_START: RegisterAccess = RegisterAccess {
    readable: false,
    writable: true,
    buffered: true,
    start_eligible: true,
    read_widths: 0,
    write_widths: WIDTH_32 | WIDTH_64,
};
const fn canonical_write_address(address: u64) -> (u64, bool) {
    if (address >= PIXEL_PIPE_BASE + START_OFFSET && address < PIXEL_PIPE_BASE + 0x1000)
        || (address >= MTE_BASE + START_OFFSET && address < MTE_BASE + 0x1000)
    {
        (address - START_OFFSET, true)
    } else {
        (address, false)
    }
}
fn register_access(
    address: u64,
    size: u8,
    direction: RegisterDirection,
) -> Result<RegisterAccess, RenderAccessError> {
    if !matches!(size, 4 | 8) || size == 4 && address & 3 != 0 || size == 8 && address & 7 != 0 {
        return Err(RenderAccessError::Access);
    }
    let access = register_descriptor(address).ok_or(RenderAccessError::Unsupported)?;
    let width = if size == 4 { WIDTH_32 } else { WIDTH_64 };
    let supported_widths = match direction {
        RegisterDirection::Read if !access.readable => return Err(RenderAccessError::Unsupported),
        RegisterDirection::Write if !access.writable => {
            return Err(RenderAccessError::Unsupported);
        }
        RegisterDirection::Read => access.read_widths,
        RegisterDirection::Write => access.write_widths,
    };
    if supported_widths & width == 0 {
        return Err(RenderAccessError::Access);
    }
    Ok(access)
}
fn register_descriptor(address: u64) -> Option<RegisterAccess> {
    if ((INTERFACE_DATA_BASE..INTERFACE_DATA_BASE + 0x200).contains(&address)
        || (INTERFACE_ADDRESS_BASE..INTERFACE_ADDRESS_BASE + 0x200).contains(&address))
        && address & 7 == 0
    {
        return Some(DIRECT_READ_WRITE_64);
    }
    if address == INTERFACE_CONTROL {
        return Some(DIRECT_READ_WRITE_32);
    }
    if address == INTERFACE_RESET || address == SET_START_POINTER {
        return Some(DIRECT_WRITE_ONLY_32);
    }
    if tlb_slot(address).is_some() {
        return Some(BUFFERED_READ_WRITE_64);
    }
    if (PIXEL_PIPE_BASE..PIXEL_PIPE_BASE + 0x200).contains(&address) {
        let offset = address - PIXEL_PIPE_BASE;
        let access = match offset {
            0x000 | 0x008 | 0x010 | 0x018 | 0x0c0 | 0x0c4 | 0x0d8 | 0x110 | 0x160 | 0x168
            | 0x170 | 0x198 | 0x1a0 | 0x1a8 | 0x1b0 | 0x1b8 | 0x1c0 | 0x1e0 | 0x1e8 => {
                BUFFERED_READ_WRITE_32_START
            }
            0x050 | 0x058 => BUFFERED_READ_WRITE_WINDOW_OFFSET,
            0x020 | 0x028 | 0x030 | 0x038 | 0x040 | 0x048 | 0x118 => BUFFERED_READ_WRITE_64_START,
            0x070 | 0x080 | 0x088 => BUFFERED_WRITE_ONLY_32_OR_64_START,
            0x060 | 0x074 | 0x078 | 0x084 | 0x08c | 0x090 | 0x094 | 0x098 | 0x0a0 | 0x0a8
            | 0x0ac | 0x0b0 | 0x0b4 | 0x0d0 | 0x0e0 | 0x0e4 | 0x0e8 | 0x0ec | 0x0f0 | 0x0f4
            | 0x0f8 | 0x0fc | 0x100 | 0x104 | 0x108 | 0x10c | 0x130 | 0x134 | 0x158 | 0x15c
            | 0x178 | 0x180 | 0x188 | 0x190 | 0x194 | 0x1f0 | 0x1f8 => BUFFERED_WRITE_ONLY_32_START,
            0x120 | 0x128 | 0x138 | 0x140 | 0x148 | 0x150 | 0x1c8 | 0x1d0 | 0x1d8 => {
                BUFFERED_WRITE_ONLY_64_START
            }
            _ => return None,
        };
        return Some(access);
    }
    if (MTE_BASE..MTE_BASE + 0x80).contains(&address) {
        return match address - MTE_BASE {
            0x00 | 0x08 | 0x18 | 0x20 | 0x28 | 0x40 | 0x48 => Some(BUFFERED_READ_WRITE_32_START),
            0x10 | 0x30 | 0x38 | 0x70 | 0x78 => Some(BUFFERED_WRITE_ONLY_32_START),
            _ => None,
        };
    }
    if address == STATUS_BASE {
        return Some(DIRECT_READ_ONLY_32);
    }
    None
}
fn tlb_slot(address: u64) -> Option<TlbSlot> {
    if address & 7 != 0 {
        return None;
    }
    if (FRAMEBUFFER_A_BASE..FRAMEBUFFER_A_BASE + 0x200).contains(&address) {
        return Some(TlbSlot::FramebufferA(
            ((address - FRAMEBUFFER_A_BASE) / 8) as usize,
        ));
    }
    if (FRAMEBUFFER_B_BASE..FRAMEBUFFER_B_BASE + 0x200).contains(&address) {
        return Some(TlbSlot::FramebufferB(
            ((address - FRAMEBUFFER_B_BASE) / 8) as usize,
        ));
    }
    if (FRAMEBUFFER_C_BASE..FRAMEBUFFER_C_BASE + 0x200).contains(&address) {
        return Some(TlbSlot::FramebufferC(
            ((address - FRAMEBUFFER_C_BASE) / 8) as usize,
        ));
    }
    if (TEXTURE_TLB_BASE..TEXTURE_TLB_BASE + 28 * 8).contains(&address) {
        return Some(TlbSlot::Texture(
            ((address - TEXTURE_TLB_BASE) / 8) as usize,
        ));
    }
    if (CID_TLB_BASE..CID_TLB_BASE + 4 * 8).contains(&address) {
        return Some(TlbSlot::Cid(((address - CID_TLB_BASE) / 8) as usize));
    }
    if (LINEAR_A_BASE..LINEAR_A_BASE + 16 * 8).contains(&address) {
        return Some(TlbSlot::LinearA(((address - LINEAR_A_BASE) / 8) as usize));
    }
    if (LINEAR_B_BASE..LINEAR_B_BASE + 16 * 8).contains(&address) {
        return Some(TlbSlot::LinearB(((address - LINEAR_B_BASE) / 8) as usize));
    }
    None
}
fn encode_interface_entry(write: RenderRegisterWrite) -> (u64, u64) {
    let relative = write.address - registers::CRIME_RENDER_BASE;
    let slot_address = relative & !7;
    let page = (slot_address >> 12) & 7;
    let offset = (slot_address & 0x7ff) >> 3;
    let (data, write_mask) = match write.size {
        4 if write.address & 4 == 0 => (write.value << 32, ADDRESS_WMASK_LOWER),
        4 => (write.value & u64::from(u32::MAX), ADDRESS_WMASK_UPPER),
        8 => (write.value, ADDRESS_WMASK_DOUBLE),
        _ => unreachable!("validated interface writes are 32-bit or 64-bit"),
    };
    let encoded_address = (if write.commit { ADDRESS_START } else { 0 })
        | (write_mask << ADDRESS_WMASK_SHIFT)
        | (page << ADDRESS_PAGE_SHIFT)
        | (offset << ADDRESS_OFFSET_SHIFT);
    (data, encoded_address)
}
fn decode_interface_entry(data: u64, address: u64) -> RenderRegisterWrite {
    let page = (address >> ADDRESS_PAGE_SHIFT) & 7;
    let offset = (address >> ADDRESS_OFFSET_SHIFT) & 0xff;
    let write_mask = (address >> ADDRESS_WMASK_SHIFT) & 3;
    let slot_address = registers::CRIME_RENDER_BASE + (page << 12) + (offset << 3);
    let (target, value, size) = match write_mask {
        ADDRESS_WMASK_UPPER => (slot_address + 4, data & u64::from(u32::MAX), 4),
        ADDRESS_WMASK_LOWER => (slot_address, data >> 32, 4),
        ADDRESS_WMASK_DOUBLE => (slot_address, data, 8),
        _ => unreachable!("hardware-generated interface entries have a write mask"),
    };
    RenderRegisterWrite {
        address: target,
        value,
        size,
        commit: address & ADDRESS_START != 0,
    }
}
fn read_register_slot(slots: &[u64; 64], offset: u64, size: u8) -> u64 {
    let value = slots[(offset / 8) as usize];
    match (size, offset & 4) {
        (4, 0) => value >> 32,
        (4, _) => value & u64::from(u32::MAX),
        (8, _) => value,
        _ => unreachable!("register widths are validated before slot access"),
    }
}
fn write_register_slot(slots: &mut [u64; 64], offset: u64, size: u8, value: u64) {
    let slot = &mut slots[(offset / 8) as usize];
    match (size, offset & 4) {
        (4, 0) => *slot = (*slot & u64::from(u32::MAX)) | (value << 32),
        (4, _) => *slot = (*slot & 0xffff_ffff_0000_0000) | (value & u64::from(u32::MAX)),
        (8, _) => *slot = value,
        _ => unreachable!("register widths are validated before slot access"),
    }
}
fn append_condition_effects(
    effects: &mut Vec<RenderInterruptEffect>,
    previous: bool,
    current: bool,
    edge_mask: u32,
    level_mask: u32,
) {
    if previous == current {
        return;
    }
    effects.push(RenderInterruptEffect {
        mask: level_mask,
        asserted: current,
    });
    if current {
        effects.push(RenderInterruptEffect {
            mask: edge_mask,
            asserted: true,
        });
    }
}
/// Applies one CRIME bitwise logic operation.
pub const fn logic_operation(operation: u8, source: u32, destination: u32) -> u32 {
    match operation & 0xf {
        0 => 0,
        1 => source & destination,
        2 => source & !destination,
        3 => source,
        4 => !source & destination,
        5 => destination,
        6 => source ^ destination,
        7 => source | destination,
        8 => !(source | destination),
        9 => !(source ^ destination),
        10 => !destination,
        11 => source | !destination,
        12 => !source,
        13 => !source | destination,
        14 => !(source & destination),
        _ => u32::MAX,
    }
}
/// Evaluates one eight-function comparison used by alpha, depth, and stencil tests.
pub const fn compare(function: u8, source: u32, reference: u32) -> bool {
    match function & 7 {
        0 => false,
        1 => source < reference,
        2 => source == reference,
        3 => source <= reference,
        4 => source > reference,
        5 => source != reference,
        6 => source >= reference,
        _ => true,
    }
}
#[cfg(test)]
mod tests;
