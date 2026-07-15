//! Typed basic-block execution shared by the interpreter and native backends.

use core::{cell::Cell, fmt};
use se_core::scheduler::FractionalClockProjection;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::cpu::mips4::alu::Mips4Alu;
use crate::cpu::mips4::cache::Mips4MemoryAccessType;
use crate::cpu::mips4::cp0::Mips4Cp0Register;
use crate::cpu::mips4::cp1::decode::{Mips4Cp1Decode, Mips4Cp1InstructionClass};
use crate::cpu::mips4::exception::{
    Mips4Exception, Mips4TrapDecision, teq, tge, tgeu, tlt, tltu, tne,
};
use crate::cpu::mips4::gpr::{MIPS4_GPR_COUNT, is_sign_extended_word};
use crate::cpu::mips4::instruction::Mips4Instruction;
use crate::cpu::mips4::instruction::decode::Mips4CpuInstruction;
use crate::cpu::mips4::instruction::requirements::Mips4InstructionRequirements;

use super::bus::{Mips4ExecutionAccessKind, Mips4ExecutionTransferSize};

use super::policy::{Mips4ExecutionPolicy, Mips4NotWordValuePolicy};

#[derive(Default)]
struct Mips4BlockKeyHasher(u64);

impl Mips4BlockKeyHasher {
    fn mix(&mut self, value: u64) {
        self.0 ^= value.wrapping_add(0x9e37_79b9_7f4a_7c15);
        self.0 = self.0.rotate_left(27).wrapping_mul(0x94d0_49bb_1331_11eb);
    }
}

impl Hasher for Mips4BlockKeyHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for chunk in &mut chunks {
            self.mix(u64::from_ne_bytes(chunk.try_into().unwrap()));
        }
        let mut tail = [0; 8];
        let remainder = chunks.remainder();
        tail[..remainder.len()].copy_from_slice(remainder);
        if !remainder.is_empty() {
            self.mix(u64::from_ne_bytes(tail));
        }
    }

    fn write_u8(&mut self, value: u8) {
        self.mix(u64::from(value));
    }

    fn write_u64(&mut self, value: u64) {
        self.mix(value);
    }

    fn write_usize(&mut self, value: usize) {
        self.mix(value as u64);
    }
}

type Mips4BlockIndexMap = HashMap<Mips4BlockKey, usize, BuildHasherDefault<Mips4BlockKeyHasher>>;

/// Maximum number of guest instructions represented by one basic block.
pub const MIPS4_BLOCK_MAX_INSTRUCTIONS: usize = 32;

/// Number of block entries before a native backend compiles a block.
pub const MIPS4_BLOCK_HOT_THRESHOLD: u64 = 256;

/// Maximum number of cached block records in one execution engine.
pub const MIPS4_BLOCK_CACHE_CAPACITY: usize = 16_384;

/// Guest operations observed at one entry before Region construction.
pub const MIPS4_REGION_HOT_THRESHOLD: u64 = 4_096;

/// Maximum number of unique block nodes in one Region.
pub const MIPS4_REGION_MAX_NODES: usize = 16;

/// Maximum number of unique guest operations in one Region.
pub const MIPS4_REGION_MAX_OPERATIONS: usize = 128;

/// Maximum number of derived Region records.
pub const MIPS4_REGION_CACHE_CAPACITY: usize = 4_096;

const MIPS4_REGION_MIN_SUCCESSOR_OBSERVATIONS: u64 = 256;
const MIPS4_REGION_DOMINANT_DIRECT_PERCENT: u64 = 75;
const MIPS4_REGION_DOMINANT_INDIRECT_PERCENT: u64 = 90;
const MIPS4_REGION_MIN_ACYCLIC_OPERATIONS: usize = 16;
const MIPS4_REGION_RETRY_OPERATIONS: u64 = 65_536;

const MIPS4_BLOCK_DISPATCH_CACHE_CAPACITY: usize = MIPS4_BLOCK_CACHE_CAPACITY;

/// Stable metadata identifying one guest instruction inside a block.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Mips4BlockInstructionMetadata {
    /// Guest virtual address of the instruction.
    pub pc: u64,

    /// Raw guest instruction bits.
    pub instruction: u32,

    /// Branch owning this instruction when it is a delay-slot instruction.
    pub delay_slot_branch_pc: Option<u64>,
}

/// Identity of a block under one instruction-translation context.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Mips4BlockKey {
    /// Current guest program counter.
    pub pc: u64,

    /// Architecturally queued next program counter.
    pub next_pc: u64,

    /// Branch owning the entry instruction when entering a delay slot.
    pub delay_slot_branch_pc: Option<u64>,

    /// Processor mode and ASID signature affecting instruction translation.
    pub fetch_context: u64,

    /// Generation of the modeled TLB contents.
    pub translation_generation: u64,

    /// Version token for an external stable code window, or zero for I-cache
    /// and transaction-fetched blocks.
    pub code_guard: u64,
}

/// Side-effect-free external code-source request produced after instruction
/// address translation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4CodeSourceRequest {
    /// Guest virtual address of the first instruction.
    pub virtual_address: u64,

    /// Translated physical address of the first instruction.
    pub physical_address: u64,

    /// Maximum byte count before the block or page limit.
    pub maximum_bytes: u8,
}

/// Stable external code-source class.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Mips4CodeGuardKind {
    /// Byte-programmable system flash.
    SystemFlash,

    /// SDRAM contents and ECC state.
    Sdram,
}

/// Versioned identity of a side-effect-free external code window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4CodeGuard {
    /// External source class.
    pub kind: Mips4CodeGuardKind,

    /// Device-local byte offset of the first byte.
    pub source_offset: u64,

    /// Source-wide mutation revision or mapping fingerprint.
    pub revision: u64,

    /// Deterministic fingerprint of the visible bytes and diagnostics.
    pub fingerprint: u64,
}

impl Mips4CodeGuard {
    /// Returns a deterministic block-key token for this source image.
    pub const fn token(self) -> u64 {
        let kind = match self.kind {
            Mips4CodeGuardKind::SystemFlash => 0x464c_4153_4800_0001,
            Mips4CodeGuardKind::Sdram => 0x5344_5241_4d00_0001,
        };
        kind ^ self.source_offset.rotate_left(11)
            ^ self.revision.rotate_left(29)
            ^ self.fingerprint.rotate_left(47)
    }
}

/// Versioned bytes and timing supplied by a stable external code source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mips4CodeWindow {
    request: Mips4CodeSourceRequest,
    guard: Mips4CodeGuard,
    bytes: [u8; MIPS4_BLOCK_MAX_INSTRUCTIONS * 4],
    byte_count: u8,
    timeline: Mips4SliceTimeline,
}

/// Per-operation simulated-time reservations for one block invocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4SliceTimeline {
    clocks: [Option<Mips4SliceClock>; 2],
    fixed_ticks_per_fetch: u64,
    len: u8,
}

/// One fractional clock consumed a fixed number of times by each fast fetch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4SliceClock {
    projection: FractionalClockProjection,
    cycles_per_fetch: u8,
}

impl Mips4SliceClock {
    /// Creates one fractional-clock contribution.
    pub const fn new(projection: FractionalClockProjection, cycles_per_fetch: u8) -> Option<Self> {
        if cycles_per_fetch == 0 {
            return None;
        }
        Some(Self {
            projection,
            cycles_per_fetch,
        })
    }
}

impl Mips4SliceTimeline {
    /// Creates a nonempty timeline from at most two fractional clocks.
    pub fn new(
        fetches: usize,
        clocks: &[Mips4SliceClock],
        fixed_ticks_per_fetch: u64,
    ) -> Option<Self> {
        if fetches == 0
            || fetches > MIPS4_BLOCK_MAX_INSTRUCTIONS
            || clocks.len() > 2
            || (clocks.is_empty() && fixed_ticks_per_fetch == 0)
        {
            return None;
        }
        let mut values = [None; 2];
        for (destination, clock) in values.iter_mut().zip(clocks.iter().copied()) {
            *destination = Some(clock);
        }
        let timeline = Self {
            clocks: values,
            fixed_ticks_per_fetch,
            len: fetches as u8,
        };
        if timeline.prefix_ticks(1)? == 0 {
            return None;
        }
        Some(timeline)
    }

    /// Returns the number of fetch reservations.
    pub fn len(&self) -> usize {
        usize::from(self.len)
    }

    /// Returns whether the timeline has no fetch reservations.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns simulated ticks consumed by a prefix of fetch reservations.
    pub fn prefix_ticks(&self, fetches: usize) -> Option<u64> {
        if fetches > self.len() {
            return None;
        }
        let mut total = self.fixed_ticks_per_fetch.checked_mul(fetches as u64)?;
        for clock in self.clocks.into_iter().flatten() {
            let cycles = u64::from(clock.cycles_per_fetch).checked_mul(fetches as u64)?;
            total = total.checked_add(clock.projection.elapsed(cycles)?.get())?;
        }
        Some(total)
    }
}

impl Mips4CodeWindow {
    /// Creates a validated external code window of at most 128 bytes.
    pub fn new(
        request: Mips4CodeSourceRequest,
        guard: Mips4CodeGuard,
        bytes: &[u8],
        timeline: Mips4SliceTimeline,
    ) -> Option<Self> {
        if bytes.len() < 4
            || bytes.len() > usize::from(request.maximum_bytes)
            || bytes.len() > MIPS4_BLOCK_MAX_INSTRUCTIONS * 4
            || !bytes.len().is_multiple_of(4)
            || timeline.len() > bytes.len() / 4
        {
            return None;
        }
        let mut source = [0; MIPS4_BLOCK_MAX_INSTRUCTIONS * 4];
        source[..bytes.len()].copy_from_slice(bytes);
        Some(Self {
            request,
            guard,
            bytes: source,
            byte_count: bytes.len() as u8,
            timeline,
        })
    }

    /// Returns the translated source request.
    pub const fn request(&self) -> Mips4CodeSourceRequest {
        self.request
    }

    /// Returns the versioned source guard.
    pub const fn guard(&self) -> Mips4CodeGuard {
        self.guard
    }

    /// Returns physical-order source bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.byte_count)]
    }

    /// Returns the number of fetches reserved for this invocation.
    pub fn fetch_count(&self) -> usize {
        self.timeline.len()
    }

    /// Returns simulated ticks consumed by the requested prefix of fetches.
    pub fn fetch_time_ticks(&self, fetches: usize) -> Option<u64> {
        self.timeline.prefix_ticks(fetches)
    }

    /// Returns the instruction index for a translated request inside this window.
    pub fn instruction_index(&self, request: Mips4CodeSourceRequest) -> Option<usize> {
        let virtual_offset = request
            .virtual_address
            .checked_sub(self.request.virtual_address)?;
        let physical_offset = request
            .physical_address
            .checked_sub(self.request.physical_address)?;
        if virtual_offset != physical_offset || !virtual_offset.is_multiple_of(4) {
            return None;
        }
        let index = usize::try_from(virtual_offset / 4).ok()?;
        (index < self.bytes().len() / 4).then_some(index)
    }
}

/// One instruction-cache line dependency of a translated block.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Mips4BlockGuardLine {
    /// Primary instruction-cache set.
    pub set: u32,

    /// Primary instruction-cache way.
    pub way: u8,

    /// Physical cache-line base stored in the way.
    pub physical_line_base: u64,

    /// Derived mutation generation of the way.
    pub generation: u64,
}

/// Complete instruction-cache visibility guard for one block.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Mips4BlockGuard {
    lines: Vec<Mips4BlockGuardLine>,
    code_source: Option<Mips4CodeGuard>,
}

impl Mips4BlockGuard {
    /// Creates an empty guard.
    pub const fn new() -> Self {
        Self {
            lines: Vec::new(),
            code_source: None,
        }
    }

    /// Creates a guard for one complete external code-source identity.
    pub const fn from_code_source(code_source: Mips4CodeGuard) -> Self {
        Self {
            lines: Vec::new(),
            code_source: Some(code_source),
        }
    }

    /// Records a cache-line dependency once.
    pub fn insert(&mut self, line: Mips4BlockGuardLine) {
        if !self
            .lines
            .iter()
            .any(|existing| existing.set == line.set && existing.way == line.way)
        {
            self.lines.push(line);
        }
    }

    /// Returns the cache-line dependencies in validation order.
    pub fn lines(&self) -> &[Mips4BlockGuardLine] {
        &self.lines
    }

    /// Returns the complete external code-source identity, when present.
    pub const fn code_source(&self) -> Option<Mips4CodeGuard> {
        self.code_source
    }
}

/// Operand read by a typed integer operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4BlockOperand {
    /// Read a guest GPR.
    Register(u8),

    /// Use a sign-extended 16-bit immediate.
    SignedImmediate(i16),

    /// Use a zero-extended 16-bit immediate.
    UnsignedImmediate(u16),
}

/// Integer operand width.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4BlockWidth {
    /// A sign-extended 32-bit word result.
    Word,

    /// A 64-bit doubleword result.
    Doubleword,
}

/// Arithmetic operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4BlockArithmetic {
    /// Addition.
    Add,

    /// Subtraction.
    Subtract,
}

/// Logical operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4BlockLogical {
    /// Bitwise AND.
    And,

    /// Bitwise OR.
    Or,

    /// Bitwise XOR.
    Xor,

    /// Bitwise NOR.
    Nor,
}

/// Shift operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4BlockShift {
    /// Logical left shift.
    Left,

    /// Logical right shift.
    RightLogical,

    /// Arithmetic right shift.
    RightArithmetic,
}

/// Shift amount source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4BlockShiftAmount {
    /// Immediate shift amount after instruction-specific adjustment.
    Immediate(u8),

    /// Low five or six bits of a GPR.
    Register(u8),
}

/// Comparison relation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4BlockComparison {
    /// Signed less-than.
    SignedLessThan,

    /// Unsigned less-than.
    UnsignedLessThan,
}

/// Integer trap relation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4BlockTrap {
    /// Equal.
    Equal,

    /// Not equal.
    NotEqual,

    /// Signed greater-than-or-equal.
    SignedGreaterThanOrEqual,

    /// Unsigned greater-than-or-equal.
    UnsignedGreaterThanOrEqual,

    /// Signed less-than.
    SignedLessThan,

    /// Unsigned less-than.
    UnsignedLessThan,
}

/// Typed operation delegated to the shared MIPS IV runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4RuntimeOperation {
    /// Integer load, store, linked access, or conditional store.
    Memory {
        /// Decoded memory operation.
        instruction: Mips4CpuInstruction,
        /// Raw fields used as normalized operands.
        raw: Mips4Instruction,
    },

    /// Integer PREF operation.
    Prefetch {
        /// Raw PREF fields after top-level classification.
        raw: Mips4Instruction,
    },

    /// Privileged CP0 or TLB operation.
    Cp0 {
        /// Raw CP0 fields after top-level classification.
        raw: Mips4Instruction,

        /// Normalized privileged operation selected by the lifter.
        operation: Mips4Cp0RuntimeOperation,
    },

    /// CP1 operation, including CP1 memory and control flow.
    Cp1 {
        /// Raw CP1 fields after top-level classification.
        raw: Mips4Instruction,

        /// Complete typed CP1 decode selected by the lifter.
        decoded: Mips4Cp1Decode,
    },

    /// Processor-specific CACHE operation.
    Cache {
        /// Raw CACHE fields after top-level classification.
        raw: Mips4Instruction,

        /// Base GPR selected by the encoding.
        base: u8,

        /// Signed address displacement.
        offset: i16,

        /// Cache selector selected by the encoding.
        selector: u8,

        /// Cache operation selected by the encoding.
        operation: u8,
    },

    /// Defined access to a non-CP0/CP1 coprocessor encoding.
    Coprocessor {
        /// Coprocessor selected by the top-level decoder.
        coprocessor: crate::cpu::mips4::exception::Mips4CoprocessorNumber,

        /// Architecture requirements selected by the lifter.
        requirements: Mips4InstructionRequirements,
    },

    /// Raise an exception selected before runtime execution.
    Raise(Mips4Exception),
}

impl Mips4RuntimeOperation {
    /// Returns the common synchronous outcome used by native runtime-call lowering.
    pub const fn synchronous_result(self) -> Mips4RuntimeResult {
        match self {
            Self::Memory { .. } | Self::Prefetch { .. } => Mips4RuntimeResult::ContinueControl,
            Self::Cp1 {
                decoded: Mips4Cp1Decode::Instruction(Mips4Cp1InstructionClass::Branch(_)),
                ..
            }
            | Self::Cp0 { .. }
            | Self::Cache { .. } => Mips4RuntimeResult::DispatchControl,
            Self::Cp1 { .. } => Mips4RuntimeResult::ContinueControl,
            Self::Coprocessor { .. } | Self::Raise(_) => Mips4RuntimeResult::Exception,
        }
    }
}

/// Stable runtime descriptor tag exposed through the native frame ABI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Mips4RuntimeOperationTag {
    /// Integer memory access.
    Memory = 1,
    /// Integer prefetch.
    Prefetch = 2,
    /// CP0, TLB, ERET, or WAIT operation.
    Cp0 = 3,
    /// CP1 operation.
    Cp1 = 4,
    /// Processor-specific CACHE operation.
    Cache = 5,
    /// CP2 or CP3 access.
    Coprocessor = 6,
    /// Preselected architectural exception.
    Raise = 7,
}

/// Stable normalized runtime operation record referenced by native blocks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct Mips4RuntimeOperationDescriptor {
    tag: Mips4RuntimeOperationTag,
    flags: u32,
    operands: [u64; 6],
}

impl Mips4RuntimeOperationDescriptor {
    fn from_operation(operation: Mips4RuntimeOperation) -> Self {
        let mut descriptor = Self {
            tag: Mips4RuntimeOperationTag::Raise,
            flags: 0,
            operands: [0; 6],
        };
        match operation {
            Mips4RuntimeOperation::Memory { instruction, raw } => {
                descriptor.tag = Mips4RuntimeOperationTag::Memory;
                descriptor.operands = [
                    instruction as u64,
                    u64::from(raw.bits()),
                    u64::from(raw.rs()),
                    u64::from(raw.rt()),
                    raw.signed_immediate() as i64 as u64,
                    0,
                ];
            }
            Mips4RuntimeOperation::Prefetch { raw } => {
                descriptor.tag = Mips4RuntimeOperationTag::Prefetch;
                descriptor.operands = [
                    u64::from(raw.bits()),
                    u64::from(raw.rs()),
                    u64::from(raw.rt()),
                    raw.signed_immediate() as i64 as u64,
                    0,
                    0,
                ];
            }
            Mips4RuntimeOperation::Cp0 { raw, operation } => {
                descriptor.tag = Mips4RuntimeOperationTag::Cp0;
                descriptor.operands[0] = u64::from(raw.bits());
                descriptor.operands[1] = cp0_operation_code(operation);
            }
            Mips4RuntimeOperation::Cp1 { raw, decoded } => {
                descriptor.tag = Mips4RuntimeOperationTag::Cp1;
                descriptor.operands[0] = u64::from(raw.bits());
                descriptor.operands[1] = match decoded {
                    Mips4Cp1Decode::Instruction(_) => 1,
                    Mips4Cp1Decode::ReservedOrUnimplementedOperation => 2,
                };
            }
            Mips4RuntimeOperation::Cache {
                raw,
                base,
                offset,
                selector,
                operation,
            } => {
                descriptor.tag = Mips4RuntimeOperationTag::Cache;
                descriptor.operands = [
                    u64::from(raw.bits()),
                    u64::from(base),
                    offset as i64 as u64,
                    u64::from(selector),
                    u64::from(operation),
                    0,
                ];
            }
            Mips4RuntimeOperation::Coprocessor {
                coprocessor,
                requirements,
            } => {
                descriptor.tag = Mips4RuntimeOperationTag::Coprocessor;
                descriptor.operands[0] = u64::from(coprocessor.number());
                descriptor.operands[1] = architecture_level_code(requirements.architecture_level);
                descriptor.operands[2] = disabled_action_code(requirements.disabled_action);
            }
            Mips4RuntimeOperation::Raise(exception) => {
                descriptor.tag = Mips4RuntimeOperationTag::Raise;
                descriptor.operands[0] = u64::from(exception.cause_code());
                if let Mips4Exception::CoprocessorUnusable { coprocessor } = exception {
                    descriptor.operands[1] = u64::from(coprocessor.number());
                }
            }
        }
        descriptor
    }

    /// Returns the stable operation tag.
    pub const fn tag(self) -> Mips4RuntimeOperationTag {
        self.tag
    }

    /// Returns stable descriptor flags.
    pub const fn flags(self) -> u32 {
        self.flags
    }

    /// Returns the normalized operand fields.
    pub const fn operands(self) -> [u64; 6] {
        self.operands
    }
}

const fn architecture_level_code(
    level: crate::cpu::mips4::instruction::requirements::Mips4ArchitectureLevel,
) -> u64 {
    use crate::cpu::mips4::instruction::requirements::Mips4ArchitectureLevel;

    match level {
        Mips4ArchitectureLevel::Mips1 => 1,
        Mips4ArchitectureLevel::Mips2 => 2,
        Mips4ArchitectureLevel::Mips3 => 3,
        Mips4ArchitectureLevel::Mips4 => 4,
    }
}

const fn disabled_action_code(
    action: crate::cpu::mips4::instruction::requirements::Mips4DisabledInstructionAction,
) -> u64 {
    use crate::cpu::mips4::instruction::requirements::Mips4DisabledInstructionAction;

    match action {
        Mips4DisabledInstructionAction::ReservedInstruction => 1,
        Mips4DisabledInstructionAction::FloatingPointUnimplemented => 2,
    }
}

const fn cp0_operation_code(operation: Mips4Cp0RuntimeOperation) -> u64 {
    match operation {
        Mips4Cp0RuntimeOperation::TransferFrom { .. } => 1,
        Mips4Cp0RuntimeOperation::TransferTo { .. } => 2,
        Mips4Cp0RuntimeOperation::TlbRead => 3,
        Mips4Cp0RuntimeOperation::TlbWriteIndexed => 4,
        Mips4Cp0RuntimeOperation::TlbWriteRandom => 5,
        Mips4Cp0RuntimeOperation::TlbProbe => 6,
        Mips4Cp0RuntimeOperation::Eret => 7,
        Mips4Cp0RuntimeOperation::Wait => 8,
        Mips4Cp0RuntimeOperation::Reserved => 9,
    }
}

/// Normalized CP0 operation used by the runtime ABI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4Cp0RuntimeOperation {
    /// Move a word or doubleword from CP0 to a GPR.
    TransferFrom {
        /// Transfer a doubleword rather than a word.
        doubleword: bool,
        /// Destination GPR.
        target: u8,
        /// Selected CP0 register, or `None` for a reserved register number.
        register: Option<Mips4Cp0Register>,
        /// Whether reserved encoding bits are clear.
        encoding_valid: bool,
    },

    /// Move a word or doubleword from a GPR to CP0.
    TransferTo {
        /// Transfer a doubleword rather than a word.
        doubleword: bool,
        /// Source GPR.
        source: u8,
        /// Selected CP0 register, or `None` for a reserved register number.
        register: Option<Mips4Cp0Register>,
        /// Whether reserved encoding bits are clear.
        encoding_valid: bool,
    },

    /// Read the indexed TLB entry into CP0 registers.
    TlbRead,

    /// Write the indexed TLB entry from CP0 registers.
    TlbWriteIndexed,

    /// Write the random TLB entry from CP0 registers.
    TlbWriteRandom,

    /// Probe the TLB using EntryHi.
    TlbProbe,

    /// Return from an exception.
    Eret,

    /// Enter the implementation-selected wait state.
    Wait,

    /// Raise Reserved Instruction after normal CP0 access checks.
    Reserved,
}

/// Result returned by one typed runtime helper invocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Mips4RuntimeResult {
    /// The operation completed and requires normal sequential retirement.
    Continue = 1,
    /// The operation completed and must dispatch after sequential retirement.
    DispatchSequential = 2,
    /// The operation installed its own control-flow state and must dispatch.
    DispatchControl = 3,
    /// The operation started an asynchronous transaction and did not retire.
    Transaction = 4,
    /// The operation entered an architectural exception and did not retire.
    Exception = 5,
    /// The operation completed WAIT and must become idle after retirement.
    Idle = 6,
    /// The runtime detected an invariant violation.
    InternalError = 7,
    /// The operation installed sequential state and may continue in the block.
    ContinueControl = 8,
    /// A proven synchronous runtime timeline cannot admit the operation.
    TimelineExhausted = 9,
}

impl Mips4RuntimeResult {
    /// Converts the stable runtime result written by native code.
    pub const fn from_code(code: u32) -> Option<Self> {
        match code {
            1 => Some(Self::Continue),
            2 => Some(Self::DispatchSequential),
            3 => Some(Self::DispatchControl),
            4 => Some(Self::Transaction),
            5 => Some(Self::Exception),
            6 => Some(Self::Idle),
            7 => Some(Self::InternalError),
            8 => Some(Self::ContinueControl),
            9 => Some(Self::TimelineExhausted),
            _ => None,
        }
    }
}

/// Stable request passed to the ABI v3 fast-memory runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct Mips4FastMemoryReadRequest {
    physical_address: u64,
    retired_boundaries: u64,
    size: u32,
    kind: u32,
    access_type: u32,
    reserved: u32,
}

impl Mips4FastMemoryReadRequest {
    pub(super) const fn new(
        physical_address: u64,
        size: Mips4ExecutionTransferSize,
        kind: Mips4ExecutionAccessKind,
        access_type: Mips4MemoryAccessType,
        retired_boundaries: u64,
    ) -> Self {
        Self {
            physical_address,
            retired_boundaries,
            size: match size {
                Mips4ExecutionTransferSize::Byte => 1,
                Mips4ExecutionTransferSize::Halfword => 2,
                Mips4ExecutionTransferSize::Word => 4,
                Mips4ExecutionTransferSize::Doubleword => 8,
            },
            kind: match kind {
                Mips4ExecutionAccessKind::InstructionFetch => 1,
                Mips4ExecutionAccessKind::DataLoad => 2,
                Mips4ExecutionAccessKind::DataStore => 3,
            },
            access_type: match access_type {
                Mips4MemoryAccessType::Uncached => 1,
                Mips4MemoryAccessType::CachedNoncoherent => 2,
                Mips4MemoryAccessType::CachedCoherent => 3,
                Mips4MemoryAccessType::ImplementationSpecific => 4,
            },
            reserved: 0,
        }
    }

    /// Returns the physical byte address.
    pub const fn physical_address(self) -> u64 {
        self.physical_address
    }

    /// Returns the transfer width in bytes.
    pub const fn size(self) -> u32 {
        self.size
    }

    /// Returns the number of earlier retirement boundaries in this slice.
    pub const fn retired_boundaries(self) -> u64 {
        self.retired_boundaries
    }

    /// Returns whether this is an uncached data load.
    pub const fn is_uncached_data_load(self) -> bool {
        self.kind == 2 && self.access_type == 1
    }
}

/// Result of one ABI v3 fast-memory read attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4FastMemoryReadResult {
    /// The machine cannot prove this request safe for direct completion.
    Unavailable,
    /// Physical byte-lane data completed synchronously.
    Complete {
        /// Physical byte-lane data.
        value: u64,
        /// Maximum total retirement boundaries admitted by the timeline.
        retirement_limit: u64,
    },
    /// The next event or deadline prevents starting the request.
    TimelineExhausted,
    /// The runtime ABI detected an invariant failure or panic.
    InternalError,
}

/// Affine side-effect-free register value available to native memory lowering.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4FastLinearReadProjection {
    /// Physical address of the register.
    pub physical_address: u64,

    /// Register value at `base_time_ticks`.
    pub base: u64,

    /// Simulated-time origin of `base`.
    pub base_time_ticks: u64,

    /// Frequency driving the affine register value.
    pub frequency_hz: u64,

    /// Simulated machine timebase frequency.
    pub timebase_hz: u64,
}

/// Reusable frequency constants for one native synchronous-memory timeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4FastMemoryParameters {
    cpu_timebase_hz: u64,
    cpu_frequency_hz: u64,
    cpu_base_ticks: u64,
    cpu_fraction_ticks: u64,
    cpu_frequency_reciprocal: u64,
    cpu_timebase_reciprocal: u64,
    bus_timebase_hz: u64,
    bus_frequency_hz: u64,
    bus_base_ticks: u64,
    bus_fraction_ticks: u64,
    bus_frequency_reciprocal: u64,
    cmi_timebase_hz: u64,
    cmi_frequency_hz: u64,
    cmi_base_ticks: u64,
    cmi_fraction_ticks: u64,
    cmi_frequency_reciprocal: u64,
    linear_read_timebase_hz: u64,
    linear_read_timebase_reciprocal: u64,
    secondary_linear_read_timebase_hz: u64,
    secondary_linear_read_timebase_reciprocal: u64,
}

impl Mips4FastMemoryParameters {
    /// Precomputes constants shared by every slice using the same clocks.
    pub fn new(
        cpu_clock: FractionalClockProjection,
        bus_clock: FractionalClockProjection,
        cmi_clock: FractionalClockProjection,
        linear_read_timebase_hz: u64,
        secondary_linear_read_timebase_hz: u64,
    ) -> Self {
        Self {
            cpu_timebase_hz: cpu_clock.timebase_hz(),
            cpu_frequency_hz: cpu_clock.frequency_hz(),
            cpu_base_ticks: cpu_clock.timebase_hz() / cpu_clock.frequency_hz(),
            cpu_fraction_ticks: cpu_clock.timebase_hz() % cpu_clock.frequency_hz(),
            cpu_frequency_reciprocal: division_reciprocal(cpu_clock.frequency_hz()),
            cpu_timebase_reciprocal: division_reciprocal(cpu_clock.timebase_hz()),
            bus_timebase_hz: bus_clock.timebase_hz(),
            bus_frequency_hz: bus_clock.frequency_hz(),
            bus_base_ticks: bus_clock.timebase_hz() / bus_clock.frequency_hz(),
            bus_fraction_ticks: bus_clock.timebase_hz() % bus_clock.frequency_hz(),
            bus_frequency_reciprocal: division_reciprocal(bus_clock.frequency_hz()),
            cmi_timebase_hz: cmi_clock.timebase_hz(),
            cmi_frequency_hz: cmi_clock.frequency_hz(),
            cmi_base_ticks: cmi_clock.timebase_hz() / cmi_clock.frequency_hz(),
            cmi_fraction_ticks: cmi_clock.timebase_hz() % cmi_clock.frequency_hz(),
            cmi_frequency_reciprocal: division_reciprocal(cmi_clock.frequency_hz()),
            linear_read_timebase_hz,
            linear_read_timebase_reciprocal: division_reciprocal(linear_read_timebase_hz),
            secondary_linear_read_timebase_hz,
            secondary_linear_read_timebase_reciprocal: division_reciprocal(
                secondary_linear_read_timebase_hz,
            ),
        }
    }

    /// Returns whether the constants describe the supplied projections.
    pub const fn matches(
        self,
        cpu_clock: FractionalClockProjection,
        bus_clock: FractionalClockProjection,
        cmi_clock: FractionalClockProjection,
        linear_read_timebase_hz: u64,
        secondary_linear_read_timebase_hz: u64,
    ) -> bool {
        self.cpu_timebase_hz == cpu_clock.timebase_hz()
            && self.cpu_frequency_hz == cpu_clock.frequency_hz()
            && self.bus_timebase_hz == bus_clock.timebase_hz()
            && self.bus_frequency_hz == bus_clock.frequency_hz()
            && self.cmi_timebase_hz == cmi_clock.timebase_hz()
            && self.cmi_frequency_hz == cmi_clock.frequency_hz()
            && self.linear_read_timebase_hz == linear_read_timebase_hz
            && self.secondary_linear_read_timebase_hz == secondary_linear_read_timebase_hz
    }
}

/// Stable ABI view used by native synchronous-memory timeline lowering.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct Mips4FastMemoryContext {
    native_linear_read_enabled: u64,
    start_time_ticks: u64,
    available_ticks: u64,
    cpu_timebase_hz: u64,
    cpu_frequency_hz: u64,
    cpu_remainder: u64,
    cpu_base_ticks: u64,
    cpu_fraction_ticks: u64,
    cpu_frequency_reciprocal: u64,
    cpu_timebase_reciprocal: u64,
    bus_timebase_hz: u64,
    bus_frequency_hz: u64,
    bus_remainder: u64,
    bus_base_ticks: u64,
    bus_fraction_ticks: u64,
    bus_frequency_reciprocal: u64,
    cmi_timebase_hz: u64,
    cmi_frequency_hz: u64,
    cmi_remainder: u64,
    cmi_base_ticks: u64,
    cmi_fraction_ticks: u64,
    cmi_frequency_reciprocal: u64,
    code_fetch_active: u64,
    code_fetch_shares_cmi: u64,
    code_fetch_fixed_ticks: u64,
    code_fetch_limit: u64,
    code_aux_timebase_hz: u64,
    code_aux_frequency_hz: u64,
    code_aux_remainder: u64,
    code_aux_base_ticks: u64,
    code_aux_fraction_ticks: u64,
    code_aux_frequency_reciprocal: u64,
    full_budget_admitted: u64,
    attempts: u64,
    completed: u64,
    cmi_completed: u64,
    last_transaction_fetch: u64,
    last_cmi_transaction_fetch: u64,
    last_delivery_ticks: u64,
    last_cmi_delivery_ticks: u64,
    linear_read_physical_address: u64,
    linear_read_base: u64,
    linear_read_base_time_ticks: u64,
    linear_read_frequency_hz: u64,
    linear_read_timebase_hz: u64,
    linear_read_timebase_reciprocal: u64,
    secondary_linear_read_physical_address: u64,
    secondary_linear_read_base: u64,
    secondary_linear_read_base_time_ticks: u64,
    secondary_linear_read_frequency_hz: u64,
    secondary_linear_read_timebase_hz: u64,
    secondary_linear_read_timebase_reciprocal: u64,
}

impl Mips4FastMemoryContext {
    /// Creates a bounded native timeline and validates overflow-free lowering.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        start_time_ticks: u64,
        available_ticks: u64,
        cpu_clock: FractionalClockProjection,
        bus_clock: FractionalClockProjection,
        cmi_clock: FractionalClockProjection,
        linear_read: Mips4FastLinearReadProjection,
        secondary_linear_read: Mips4FastLinearReadProjection,
        parameters: Mips4FastMemoryParameters,
        full_budget_admitted: bool,
    ) -> Self {
        let end_time = start_time_ticks.checked_add(available_ticks);
        let latest_linear_elapsed = end_time.map(|end| {
            end.saturating_sub(linear_read.base_time_ticks)
                .checked_mul(linear_read.frequency_hz)
        });
        let latest_secondary_linear_elapsed = end_time.map(|end| {
            end.saturating_sub(secondary_linear_read.base_time_ticks)
                .checked_mul(secondary_linear_read.frequency_hz)
        });
        let native_linear_read_enabled = u64::from(
            available_ticks != 0
                && cpu_clock.frequency_hz() > 1
                && cpu_clock.timebase_hz() > 1
                && bus_clock.frequency_hz() > 1
                && cmi_clock.frequency_hz() > 1
                && linear_read.timebase_hz > 1
                && secondary_linear_read.timebase_hz > 1
                && cpu_clock.elapsed(256).is_some()
                && bus_clock.elapsed(512).is_some()
                && cmi_clock.elapsed(512).is_some()
                && available_ticks
                    .checked_add(1)
                    .and_then(|ticks| ticks.checked_mul(cpu_clock.frequency_hz()))
                    .is_some()
                && latest_linear_elapsed.flatten().is_some()
                && latest_secondary_linear_elapsed.flatten().is_some()
                && linear_read.timebase_hz != 0
                && secondary_linear_read.timebase_hz != 0
                && parameters.matches(
                    cpu_clock,
                    bus_clock,
                    cmi_clock,
                    linear_read.timebase_hz,
                    secondary_linear_read.timebase_hz,
                ),
        );
        Self {
            native_linear_read_enabled,
            start_time_ticks,
            available_ticks,
            cpu_timebase_hz: cpu_clock.timebase_hz(),
            cpu_frequency_hz: cpu_clock.frequency_hz(),
            cpu_remainder: cpu_clock.remainder(),
            cpu_base_ticks: parameters.cpu_base_ticks,
            cpu_fraction_ticks: parameters.cpu_fraction_ticks,
            cpu_frequency_reciprocal: parameters.cpu_frequency_reciprocal,
            cpu_timebase_reciprocal: parameters.cpu_timebase_reciprocal,
            bus_timebase_hz: bus_clock.timebase_hz(),
            bus_frequency_hz: bus_clock.frequency_hz(),
            bus_remainder: bus_clock.remainder(),
            bus_base_ticks: parameters.bus_base_ticks,
            bus_fraction_ticks: parameters.bus_fraction_ticks,
            bus_frequency_reciprocal: parameters.bus_frequency_reciprocal,
            cmi_timebase_hz: cmi_clock.timebase_hz(),
            cmi_frequency_hz: cmi_clock.frequency_hz(),
            cmi_remainder: cmi_clock.remainder(),
            cmi_base_ticks: parameters.cmi_base_ticks,
            cmi_fraction_ticks: parameters.cmi_fraction_ticks,
            cmi_frequency_reciprocal: parameters.cmi_frequency_reciprocal,
            code_fetch_active: 0,
            code_fetch_shares_cmi: 0,
            code_fetch_fixed_ticks: 0,
            code_fetch_limit: 0,
            code_aux_timebase_hz: cmi_clock.timebase_hz(),
            code_aux_frequency_hz: cmi_clock.frequency_hz(),
            code_aux_remainder: cmi_clock.remainder(),
            code_aux_base_ticks: parameters.cmi_base_ticks,
            code_aux_fraction_ticks: parameters.cmi_fraction_ticks,
            code_aux_frequency_reciprocal: parameters.cmi_frequency_reciprocal,
            full_budget_admitted: u64::from(full_budget_admitted),
            attempts: 0,
            completed: 0,
            cmi_completed: 0,
            last_transaction_fetch: 0,
            last_cmi_transaction_fetch: 0,
            last_delivery_ticks: 0,
            last_cmi_delivery_ticks: 0,
            linear_read_physical_address: linear_read.physical_address,
            linear_read_base: linear_read.base,
            linear_read_base_time_ticks: linear_read.base_time_ticks,
            linear_read_frequency_hz: linear_read.frequency_hz,
            linear_read_timebase_hz: linear_read.timebase_hz,
            linear_read_timebase_reciprocal: parameters.linear_read_timebase_reciprocal,
            secondary_linear_read_physical_address: secondary_linear_read.physical_address,
            secondary_linear_read_base: secondary_linear_read.base,
            secondary_linear_read_base_time_ticks: secondary_linear_read.base_time_ticks,
            secondary_linear_read_frequency_hz: secondary_linear_read.frequency_hz,
            secondary_linear_read_timebase_hz: secondary_linear_read.timebase_hz,
            secondary_linear_read_timebase_reciprocal: parameters
                .secondary_linear_read_timebase_reciprocal,
        }
    }

    /// Returns the CPU clock projection captured at slice entry.
    pub const fn cpu_clock(&self) -> FractionalClockProjection {
        FractionalClockProjection::new(
            self.cpu_timebase_hz,
            self.cpu_frequency_hz,
            self.cpu_remainder,
        )
    }

    /// Returns the SysAD clock projection captured at slice entry.
    pub const fn bus_clock(&self) -> FractionalClockProjection {
        FractionalClockProjection::new(
            self.bus_timebase_hz,
            self.bus_frequency_hz,
            self.bus_remainder,
        )
    }

    /// Returns the CMI clock projection captured at slice entry.
    pub const fn cmi_clock(&self) -> FractionalClockProjection {
        FractionalClockProjection::new(
            self.cmi_timebase_hz,
            self.cmi_frequency_hz,
            self.cmi_remainder,
        )
    }

    /// Adds the stable code-source clock consumed before each guest operation.
    pub fn configure_code_fetch_timeline(
        &mut self,
        auxiliary_clock: FractionalClockProjection,
        fixed_ticks_per_fetch: u64,
        shares_cmi_clock: bool,
        fetch_limit: u64,
    ) -> bool {
        if self.code_fetch_active != 0
            || fetch_limit == 0
            || fetch_limit > 256
            || auxiliary_clock.frequency_hz() <= 1
            || auxiliary_clock.timebase_hz() <= 1
            || auxiliary_clock.elapsed(512).is_none()
            || (shares_cmi_clock && auxiliary_clock != self.cmi_clock())
        {
            return false;
        }
        self.code_fetch_active = 1;
        self.code_fetch_shares_cmi = u64::from(shares_cmi_clock);
        self.code_fetch_fixed_ticks = fixed_ticks_per_fetch;
        self.code_fetch_limit = fetch_limit;
        self.code_aux_timebase_hz = auxiliary_clock.timebase_hz();
        self.code_aux_frequency_hz = auxiliary_clock.frequency_hz();
        self.code_aux_remainder = auxiliary_clock.remainder();
        self.code_aux_base_ticks = auxiliary_clock.timebase_hz() / auxiliary_clock.frequency_hz();
        self.code_aux_fraction_ticks =
            auxiliary_clock.timebase_hz() % auxiliary_clock.frequency_hz();
        self.code_aux_frequency_reciprocal = division_reciprocal(auxiliary_clock.frequency_hz());
        true
    }

    /// Returns whether stable code fetches are folded into this timeline.
    pub const fn code_fetch_active(&self) -> bool {
        self.code_fetch_active != 0
    }

    /// Returns whether stable code fetches consume the same CMI clock.
    pub const fn code_fetch_shares_cmi(&self) -> bool {
        self.code_fetch_shares_cmi != 0
    }

    /// Returns the auxiliary stable-code clock projection.
    pub const fn code_aux_clock(&self) -> FractionalClockProjection {
        FractionalClockProjection::new(
            self.code_aux_timebase_hz,
            self.code_aux_frequency_hz,
            self.code_aux_remainder,
        )
    }

    /// Returns fixed simulated ticks consumed by every stable code fetch.
    pub const fn code_fetch_fixed_ticks(&self) -> u64 {
        self.code_fetch_fixed_ticks
    }

    /// Returns the maximum stable fetch prefix bound to this invocation.
    pub const fn code_fetch_limit(&self) -> u64 {
        self.code_fetch_limit
    }

    /// Returns the slice start time in simulated ticks.
    pub const fn start_time_ticks(&self) -> u64 {
        self.start_time_ticks
    }

    /// Returns the strict upper simulated-time bound for the slice.
    pub const fn available_ticks(&self) -> u64 {
        self.available_ticks
    }

    /// Returns whether slice planning proved the complete boundary budget fits.
    pub const fn full_budget_admitted(&self) -> bool {
        self.full_budget_admitted != 0
    }

    /// Tightens the strict simulated-time bound without extending it.
    pub fn limit_available_ticks(&mut self, available_ticks: u64) {
        if available_ticks < self.available_ticks {
            self.available_ticks = available_ticks;
            self.full_budget_admitted = 0;
        }
    }

    /// Records one attempted native or helper memory completion.
    pub fn record_attempt(&mut self) {
        self.attempts = self.attempts.saturating_add(1);
    }

    /// Records one completed memory transaction and its delivery offset.
    pub fn record_completion(&mut self, delivery_ticks: u64, fetches: u64) {
        self.completed = self.completed.saturating_add(1);
        self.last_delivery_ticks = delivery_ticks;
        self.last_transaction_fetch = fetches;
    }

    /// Returns attempted synchronous transactions.
    pub const fn attempts(&self) -> u64 {
        self.attempts
    }

    /// Returns completed synchronous transactions.
    pub const fn completed(&self) -> u64 {
        self.completed
    }

    /// Returns completed synchronous transactions routed over CMI.
    pub const fn cmi_completed(&self) -> u64 {
        self.cmi_completed
    }

    /// Returns the last completed request-delivery offset.
    pub const fn last_delivery_ticks(&self) -> u64 {
        self.last_delivery_ticks
    }

    /// Returns the last MACE delivery offset in simulation ticks.
    pub const fn last_cmi_delivery_ticks(&self) -> u64 {
        self.last_cmi_delivery_ticks
    }

    /// Returns the fetch index of the last completed synchronous transaction.
    pub const fn last_transaction_fetch(&self) -> u64 {
        self.last_transaction_fetch
    }

    /// Returns the fetch index of the last completed CMI transaction.
    pub const fn last_cmi_transaction_fetch(&self) -> u64 {
        self.last_cmi_transaction_fetch
    }
}

const fn division_reciprocal(divisor: u64) -> u64 {
    if divisor <= 1 {
        return 0;
    }
    ((u64::MAX as u128 + 1) / divisor as u128) as u64
}

/// Byte offsets of stable fields used by native fast-memory lowering.
pub const MIPS4_FAST_MEMORY_NATIVE_ENABLED_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, native_linear_read_enabled) as i32;
pub const MIPS4_FAST_MEMORY_START_TIME_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, start_time_ticks) as i32;
pub const MIPS4_FAST_MEMORY_AVAILABLE_TICKS_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, available_ticks) as i32;
pub const MIPS4_FAST_MEMORY_CPU_TIMEBASE_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, cpu_timebase_hz) as i32;
pub const MIPS4_FAST_MEMORY_CPU_FREQUENCY_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, cpu_frequency_hz) as i32;
pub const MIPS4_FAST_MEMORY_CPU_REMAINDER_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, cpu_remainder) as i32;
pub const MIPS4_FAST_MEMORY_CPU_BASE_TICKS_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, cpu_base_ticks) as i32;
pub const MIPS4_FAST_MEMORY_CPU_FRACTION_TICKS_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, cpu_fraction_ticks) as i32;
pub const MIPS4_FAST_MEMORY_CPU_FREQUENCY_RECIPROCAL_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, cpu_frequency_reciprocal) as i32;
pub const MIPS4_FAST_MEMORY_CPU_TIMEBASE_RECIPROCAL_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, cpu_timebase_reciprocal) as i32;
pub const MIPS4_FAST_MEMORY_BUS_TIMEBASE_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, bus_timebase_hz) as i32;
pub const MIPS4_FAST_MEMORY_BUS_FREQUENCY_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, bus_frequency_hz) as i32;
pub const MIPS4_FAST_MEMORY_BUS_REMAINDER_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, bus_remainder) as i32;
pub const MIPS4_FAST_MEMORY_BUS_BASE_TICKS_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, bus_base_ticks) as i32;
pub const MIPS4_FAST_MEMORY_BUS_FRACTION_TICKS_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, bus_fraction_ticks) as i32;
pub const MIPS4_FAST_MEMORY_BUS_FREQUENCY_RECIPROCAL_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, bus_frequency_reciprocal) as i32;
pub const MIPS4_FAST_MEMORY_CMI_FREQUENCY_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, cmi_frequency_hz) as i32;
pub const MIPS4_FAST_MEMORY_CMI_REMAINDER_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, cmi_remainder) as i32;
pub const MIPS4_FAST_MEMORY_CMI_BASE_TICKS_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, cmi_base_ticks) as i32;
pub const MIPS4_FAST_MEMORY_CMI_FRACTION_TICKS_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, cmi_fraction_ticks) as i32;
pub const MIPS4_FAST_MEMORY_CMI_FREQUENCY_RECIPROCAL_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, cmi_frequency_reciprocal) as i32;
pub const MIPS4_FAST_MEMORY_CODE_FETCH_ACTIVE_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, code_fetch_active) as i32;
pub const MIPS4_FAST_MEMORY_CODE_FETCH_SHARES_CMI_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, code_fetch_shares_cmi) as i32;
pub const MIPS4_FAST_MEMORY_CODE_FETCH_FIXED_TICKS_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, code_fetch_fixed_ticks) as i32;
pub const MIPS4_FAST_MEMORY_CODE_FETCH_LIMIT_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, code_fetch_limit) as i32;
pub const MIPS4_FAST_MEMORY_CODE_AUX_BASE_TICKS_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, code_aux_base_ticks) as i32;
pub const MIPS4_FAST_MEMORY_CODE_AUX_FRACTION_TICKS_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, code_aux_fraction_ticks) as i32;
pub const MIPS4_FAST_MEMORY_CODE_AUX_FREQUENCY_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, code_aux_frequency_hz) as i32;
pub const MIPS4_FAST_MEMORY_CODE_AUX_FREQUENCY_RECIPROCAL_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, code_aux_frequency_reciprocal) as i32;
pub const MIPS4_FAST_MEMORY_CODE_AUX_REMAINDER_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, code_aux_remainder) as i32;
pub const MIPS4_FAST_MEMORY_FULL_BUDGET_ADMITTED_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, full_budget_admitted) as i32;
pub const MIPS4_FAST_MEMORY_ATTEMPTS_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, attempts) as i32;
pub const MIPS4_FAST_MEMORY_COMPLETED_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, completed) as i32;
pub const MIPS4_FAST_MEMORY_CMI_COMPLETED_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, cmi_completed) as i32;
pub const MIPS4_FAST_MEMORY_LAST_TRANSACTION_FETCH_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, last_transaction_fetch) as i32;
pub const MIPS4_FAST_MEMORY_LAST_CMI_TRANSACTION_FETCH_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, last_cmi_transaction_fetch) as i32;
pub const MIPS4_FAST_MEMORY_LAST_DELIVERY_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, last_delivery_ticks) as i32;
pub const MIPS4_FAST_MEMORY_LAST_CMI_DELIVERY_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, last_cmi_delivery_ticks) as i32;
pub const MIPS4_FAST_MEMORY_LINEAR_ADDRESS_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, linear_read_physical_address) as i32;
pub const MIPS4_FAST_MEMORY_LINEAR_BASE_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, linear_read_base) as i32;
pub const MIPS4_FAST_MEMORY_LINEAR_BASE_TIME_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, linear_read_base_time_ticks) as i32;
pub const MIPS4_FAST_MEMORY_LINEAR_FREQUENCY_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, linear_read_frequency_hz) as i32;
pub const MIPS4_FAST_MEMORY_LINEAR_TIMEBASE_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, linear_read_timebase_hz) as i32;
pub const MIPS4_FAST_MEMORY_LINEAR_TIMEBASE_RECIPROCAL_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, linear_read_timebase_reciprocal) as i32;
pub const MIPS4_FAST_MEMORY_SECONDARY_LINEAR_ADDRESS_OFFSET: i32 = core::mem::offset_of!(
    Mips4FastMemoryContext,
    secondary_linear_read_physical_address
) as i32;
pub const MIPS4_FAST_MEMORY_SECONDARY_LINEAR_BASE_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, secondary_linear_read_base) as i32;
pub const MIPS4_FAST_MEMORY_SECONDARY_LINEAR_BASE_TIME_OFFSET: i32 = core::mem::offset_of!(
    Mips4FastMemoryContext,
    secondary_linear_read_base_time_ticks
) as i32;
pub const MIPS4_FAST_MEMORY_SECONDARY_LINEAR_FREQUENCY_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, secondary_linear_read_frequency_hz) as i32;
pub const MIPS4_FAST_MEMORY_SECONDARY_LINEAR_TIMEBASE_OFFSET: i32 =
    core::mem::offset_of!(Mips4FastMemoryContext, secondary_linear_read_timebase_hz) as i32;
pub const MIPS4_FAST_MEMORY_SECONDARY_LINEAR_TIMEBASE_RECIPROCAL_OFFSET: i32 = core::mem::offset_of!(
    Mips4FastMemoryContext,
    secondary_linear_read_timebase_reciprocal
) as i32;

/// Machine-owned runtime used for proven synchronous memory completion.
pub trait Mips4FastMemoryRuntime {
    /// Attempts one already translated, aligned read.
    fn read(&mut self, request: Mips4FastMemoryReadRequest) -> Mips4FastMemoryReadResult;

    /// Returns logical transactions completed since this runtime was created.
    fn completed_transactions(&self) -> u64 {
        0
    }

    /// Returns the physical half-open range admitted by the native read entry.
    fn native_read_physical_range(&self) -> Option<(u64, u64)> {
        None
    }

    /// Returns the stable native timeline context for this slice.
    fn native_context(&mut self) -> Option<&mut Mips4FastMemoryContext> {
        None
    }
}

#[derive(Clone, Copy)]
#[repr(C)]
struct Mips4FastMemoryAbiResponse {
    outcome: u32,
    reserved: u32,
    value: u64,
    retirement_limit: u64,
}

type Mips4FastMemoryReadCall =
    unsafe extern "C" fn(*mut (), *const Mips4FastMemoryReadRequest) -> Mips4FastMemoryAbiResponse;
type Mips4FastMemoryFrameReadCall =
    unsafe extern "C" fn(*mut (), *mut Mips4BlockFrame, u64, u32) -> u32;

/// Non-serialized ABI v3 runtime vtable installed for one CPU slice.
#[derive(Clone, Copy)]
pub struct Mips4RuntimeAbiV3 {
    context: *mut (),
    native_context: *mut Mips4FastMemoryContext,
    read: Mips4FastMemoryReadCall,
    frame_read: Mips4FastMemoryFrameReadCall,
    read_start: u64,
    read_end: u64,
}

impl Mips4RuntimeAbiV3 {
    pub(super) fn new<R>(runtime: &mut R) -> Self
    where
        R: Mips4FastMemoryRuntime,
    {
        let (read_start, read_end) = runtime.native_read_physical_range().unwrap_or((0, 0));
        let native_context = runtime
            .native_context()
            .map_or(core::ptr::null_mut(), core::ptr::from_mut);
        Self {
            context: core::ptr::from_mut(runtime).cast(),
            native_context,
            read: mips4_fast_memory_read_trampoline::<R>,
            frame_read: mips4_fast_memory_frame_read_trampoline::<R>,
            read_start,
            read_end,
        }
    }

    pub(super) fn read(self, request: Mips4FastMemoryReadRequest) -> Mips4FastMemoryReadResult {
        // SAFETY: The CPU installs a live runtime for exactly one synchronous
        // slice and clears the binding before returning to the machine.
        let response = unsafe { (self.read)(self.context, &request) };
        match response.outcome {
            0 => Mips4FastMemoryReadResult::Unavailable,
            1 => Mips4FastMemoryReadResult::Complete {
                value: response.value,
                retirement_limit: response.retirement_limit,
            },
            2 => Mips4FastMemoryReadResult::TimelineExhausted,
            _ => Mips4FastMemoryReadResult::InternalError,
        }
    }
}

extern "C" fn mips4_fast_memory_frame_read_trampoline<R>(
    context: *mut (),
    frame: *mut Mips4BlockFrame,
    physical_address: u64,
    size: u32,
) -> u32
where
    R: Mips4FastMemoryRuntime,
{
    catch_unwind(AssertUnwindSafe(|| {
        if context.is_null() || frame.is_null() {
            return 3;
        }
        // SAFETY: Native execution installs a live runtime and uniquely borrowed
        // frame for exactly the duration of this call.
        let runtime = unsafe { &mut *context.cast::<R>() };
        let frame = unsafe { &mut *frame };
        let size = match size {
            1 => Mips4ExecutionTransferSize::Byte,
            2 => Mips4ExecutionTransferSize::Halfword,
            4 => Mips4ExecutionTransferSize::Word,
            8 => Mips4ExecutionTransferSize::Doubleword,
            _ => return 3,
        };
        let request = Mips4FastMemoryReadRequest::new(
            physical_address,
            size,
            Mips4ExecutionAccessKind::DataLoad,
            Mips4MemoryAccessType::Uncached,
            frame.retired,
        );
        match runtime.read(request) {
            Mips4FastMemoryReadResult::Unavailable => 0,
            Mips4FastMemoryReadResult::Complete {
                value,
                retirement_limit,
            } => {
                let remaining = retirement_limit.saturating_sub(frame.retired);
                if remaining == 0 {
                    return 2;
                }
                frame.limit_budget(remaining);
                frame.runtime_value = value;
                1
            }
            Mips4FastMemoryReadResult::TimelineExhausted => 2,
            Mips4FastMemoryReadResult::InternalError => 3,
        }
    }))
    .unwrap_or(3)
}

extern "C" fn mips4_fast_memory_read_trampoline<R>(
    context: *mut (),
    request: *const Mips4FastMemoryReadRequest,
) -> Mips4FastMemoryAbiResponse
where
    R: Mips4FastMemoryRuntime,
{
    let result = catch_unwind(AssertUnwindSafe(|| {
        if context.is_null() || request.is_null() {
            return Mips4FastMemoryReadResult::InternalError;
        }
        // SAFETY: The binding owns a live runtime and request for this call.
        let runtime = unsafe { &mut *context.cast::<R>() };
        let request = unsafe { *request };
        runtime.read(request)
    }))
    .unwrap_or(Mips4FastMemoryReadResult::InternalError);
    match result {
        Mips4FastMemoryReadResult::Unavailable => Mips4FastMemoryAbiResponse {
            outcome: 0,
            reserved: 0,
            value: 0,
            retirement_limit: 0,
        },
        Mips4FastMemoryReadResult::Complete {
            value,
            retirement_limit,
        } => Mips4FastMemoryAbiResponse {
            outcome: 1,
            reserved: 0,
            value,
            retirement_limit,
        },
        Mips4FastMemoryReadResult::TimelineExhausted => Mips4FastMemoryAbiResponse {
            outcome: 2,
            reserved: 0,
            value: 0,
            retirement_limit: 0,
        },
        Mips4FastMemoryReadResult::InternalError => Mips4FastMemoryAbiResponse {
            outcome: 3,
            reserved: 0,
            value: 0,
            retirement_limit: 0,
        },
    }
}

/// Shared runtime semantics invoked by interpreted and native blocks.
pub trait Mips4BlockRuntime {
    /// Executes one normalized runtime operation without decoding it again.
    fn execute(
        &mut self,
        frame: &mut Mips4BlockFrame,
        operation: Mips4RuntimeOperation,
    ) -> Mips4RuntimeResult;

    /// Returns the machine-owned fast-memory ABI available for this slice.
    fn runtime_abi_v3(&mut self) -> Option<Mips4RuntimeAbiV3> {
        None
    }

    /// Returns whether ordinary integer memory accesses use big-endian byte order.
    fn runtime_memory_big_endian(&self) -> bool {
        false
    }

    /// Checks whether a cached instruction-source guard remains visible.
    fn block_guard_valid(&self, _guard: &Mips4BlockGuard) -> bool {
        true
    }

    /// Returns an epoch changed by any mutation that can affect cached guards.
    fn block_guard_epoch(&self) -> u64 {
        0
    }
}

/// Typed operation performed by one sequential guest instruction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4BlockOperation {
    /// Add or subtract two integer operands.
    Arithmetic {
        /// Arithmetic operation.
        operation: Mips4BlockArithmetic,
        /// Operand width.
        width: Mips4BlockWidth,
        /// Raise ArithmeticOverflow instead of wrapping.
        trap_on_overflow: bool,
        /// Ignore the instruction when required word operands are invalid.
        noop_on_invalid_word: bool,
        /// Destination GPR.
        destination: u8,
        /// Left source GPR.
        lhs: u8,
        /// Right source.
        rhs: Mips4BlockOperand,
    },

    /// Perform a bitwise operation.
    Logical {
        /// Logical operation.
        operation: Mips4BlockLogical,
        /// Destination GPR.
        destination: u8,
        /// Left source GPR.
        lhs: u8,
        /// Right source.
        rhs: Mips4BlockOperand,
    },

    /// Load an immediate into the high half of a sign-extended word.
    LoadUpperImmediate {
        /// Destination GPR.
        destination: u8,
        /// Immediate value.
        immediate: u16,
    },

    /// Shift an integer value.
    Shift {
        /// Shift operation.
        operation: Mips4BlockShift,
        /// Operand width.
        width: Mips4BlockWidth,
        /// Ignore the instruction when the word operand is invalid.
        noop_on_invalid_word: bool,
        /// Destination GPR.
        destination: u8,
        /// Value source GPR.
        value: u8,
        /// Shift amount source.
        amount: Mips4BlockShiftAmount,
    },

    /// Compare two integer values and write zero or one.
    Compare {
        /// Comparison relation.
        comparison: Mips4BlockComparison,
        /// Destination GPR.
        destination: u8,
        /// Left source GPR.
        lhs: u8,
        /// Right source.
        rhs: Mips4BlockOperand,
    },

    /// Multiply two values into HI and LO.
    Multiply {
        /// Operand width.
        width: Mips4BlockWidth,
        /// Whether operands are signed.
        signed: bool,
        /// Ignore the instruction when required word operands are invalid.
        noop_on_invalid_word: bool,
        /// Left source GPR.
        lhs: u8,
        /// Right source GPR.
        rhs: u8,
    },

    /// Divide two values into HI and LO.
    Divide {
        /// Operand width.
        width: Mips4BlockWidth,
        /// Whether operands are signed.
        signed: bool,
        /// Ignore the instruction when required word operands are invalid.
        noop_on_invalid_word: bool,
        /// Left source GPR.
        lhs: u8,
        /// Right source GPR.
        rhs: u8,
    },

    /// Copy HI or LO into a GPR.
    MoveFromSpecial {
        /// Read HI when true and LO when false.
        high: bool,
        /// Destination GPR.
        destination: u8,
    },

    /// Copy a GPR into HI or LO.
    MoveToSpecial {
        /// Write HI when true and LO when false.
        high: bool,
        /// Source GPR.
        source: u8,
    },

    /// Conditionally copy one GPR into another.
    ConditionalMove {
        /// Move when the condition is zero instead of nonzero.
        when_zero: bool,
        /// Destination GPR.
        destination: u8,
        /// Value source GPR.
        source: u8,
        /// Condition source GPR.
        condition: u8,
    },

    /// Raise Trap when a relation holds.
    Trap {
        /// Trap relation.
        trap: Mips4BlockTrap,
        /// Left source GPR.
        lhs: u8,
        /// Right source.
        rhs: Mips4BlockOperand,
    },

    /// Raise an unconditional architectural exception.
    Exception(Mips4BlockException),

    /// Execute one typed operation through the shared runtime ABI.
    Runtime(Mips4RuntimeOperation),

    /// Retire without another architectural effect.
    NoOperation,
}

/// One sequential guest instruction in typed block form.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4BlockInstruction {
    /// Stable guest metadata.
    pub metadata: Mips4BlockInstructionMetadata,

    /// Typed architectural operation.
    pub operation: Mips4BlockOperation,

    /// Explicit retirement marker for the guest instruction.
    pub retire: Mips4BlockRetire,
}

/// Explicit retirement marker attached to one guest instruction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4BlockRetire {
    /// Guest PC retired by this marker.
    pub pc: u64,
}

/// Conditional branch relation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4BlockBranchCondition {
    /// Always take the control transfer.
    Always,

    /// Compare two GPRs for equality.
    Equal { lhs: u8, rhs: u8 },

    /// Compare two GPRs for inequality.
    NotEqual { lhs: u8, rhs: u8 },

    /// Test a GPR as signed less-than zero.
    LessThanZero { source: u8 },

    /// Test a GPR as signed greater-than-or-equal to zero.
    GreaterThanOrEqualZero { source: u8 },

    /// Test a GPR as signed less-than-or-equal to zero.
    LessThanOrEqualZero { source: u8 },

    /// Test a GPR as signed greater-than zero.
    GreaterThanZero { source: u8 },
}

/// Branch target source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4BlockBranchTarget {
    /// Statically computed guest address.
    Direct(u64),

    /// Guest address read from a GPR.
    Register(u8),
}

/// Control transfer ending one block.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4BlockBranch {
    /// Metadata for the branch instruction.
    pub metadata: Mips4BlockInstructionMetadata,

    /// Condition controlling the transfer.
    pub condition: Mips4BlockBranchCondition,

    /// Taken target.
    pub target: Mips4BlockBranchTarget,

    /// Nullify the delay slot when the condition is false.
    pub likely: bool,

    /// Optional link-register destination.
    pub link: Option<u8>,

    /// Explicit retirement marker for the branch instruction.
    pub retire: Mips4BlockRetire,
}

/// Result of lifting one decoded CPU instruction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4BlockLiftedInstruction {
    /// Sequential instruction that may be added to a block body.
    Sequential(Mips4BlockInstruction),

    /// Control transfer that must terminate the block.
    Branch(Mips4BlockBranch),
}

/// Immutable translated MIPS IV basic block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mips4Block {
    key: Mips4BlockKey,
    guard: Mips4BlockGuard,
    body: Vec<Mips4BlockInstruction>,
    branch: Option<Mips4BlockBranch>,
    delay_slot: Option<Mips4BlockInstruction>,
    terminated: bool,
}

impl Mips4Block {
    /// Creates an empty block for incremental construction.
    pub const fn new(key: Mips4BlockKey, guard: Mips4BlockGuard) -> Self {
        Self {
            key,
            guard,
            body: Vec::new(),
            branch: None,
            delay_slot: None,
            terminated: false,
        }
    }

    /// Returns the entry identity.
    pub const fn key(&self) -> Mips4BlockKey {
        self.key
    }

    /// Returns the instruction-cache guard.
    pub const fn guard(&self) -> &Mips4BlockGuard {
        &self.guard
    }

    /// Records one instruction-cache line dependency while constructing the block.
    pub fn add_guard_line(&mut self, line: Mips4BlockGuardLine) {
        self.guard.insert(line);
    }

    /// Returns sequential instructions before the terminator.
    pub fn body(&self) -> &[Mips4BlockInstruction] {
        &self.body
    }

    /// Returns the optional control transfer.
    pub const fn branch(&self) -> Option<Mips4BlockBranch> {
        self.branch
    }

    /// Returns the optional branch delay-slot instruction.
    pub const fn delay_slot(&self) -> Option<Mips4BlockInstruction> {
        self.delay_slot
    }

    /// Returns the number of represented guest instructions.
    pub fn instruction_count(&self) -> usize {
        self.body.len()
            + usize::from(self.branch.is_some())
            + usize::from(self.delay_slot.is_some())
    }

    fn runtime_operations(&self) -> Vec<Mips4RuntimeOperation> {
        self.body
            .iter()
            .chain(self.delay_slot.iter())
            .filter_map(|instruction| match instruction.operation {
                Mips4BlockOperation::Runtime(operation) => Some(operation),
                _ => None,
            })
            .collect()
    }

    /// Adds a sequential instruction before the block terminator.
    pub fn push(&mut self, instruction: Mips4BlockInstruction) -> Result<(), Mips4BlockBuildError> {
        if self.terminated {
            return Err(Mips4BlockBuildError::InstructionAfterTerminator);
        }
        if self.instruction_count() == MIPS4_BLOCK_MAX_INSTRUCTIONS {
            return Err(Mips4BlockBuildError::InstructionLimit);
        }
        self.check_page(instruction.metadata.pc)?;
        self.body.push(instruction);
        Ok(())
    }

    /// Installs the terminating branch and its supported delay-slot instruction.
    pub fn terminate_with_branch(
        &mut self,
        branch: Mips4BlockBranch,
        delay_slot: Mips4BlockInstruction,
    ) -> Result<(), Mips4BlockBuildError> {
        if self.terminated {
            return Err(Mips4BlockBuildError::DuplicateTerminator);
        }
        if self.instruction_count() + 2 > MIPS4_BLOCK_MAX_INSTRUCTIONS {
            return Err(Mips4BlockBuildError::InstructionLimit);
        }
        if delay_slot.metadata.pc != branch.metadata.pc.wrapping_add(4)
            || delay_slot.metadata.delay_slot_branch_pc != Some(branch.metadata.pc)
        {
            return Err(Mips4BlockBuildError::InvalidDelaySlot);
        }
        self.check_page(branch.metadata.pc)?;
        self.check_page(delay_slot.metadata.pc)?;
        self.branch = Some(branch);
        self.delay_slot = Some(delay_slot);
        self.terminated = true;
        Ok(())
    }

    /// Ends a block immediately after a branch, before fetching its delay slot.
    pub fn terminate_at_branch(
        &mut self,
        branch: Mips4BlockBranch,
    ) -> Result<(), Mips4BlockBuildError> {
        if self.terminated {
            return Err(Mips4BlockBuildError::DuplicateTerminator);
        }
        if self.instruction_count() == MIPS4_BLOCK_MAX_INSTRUCTIONS {
            return Err(Mips4BlockBuildError::InstructionLimit);
        }
        self.check_page(branch.metadata.pc)?;
        self.branch = Some(branch);
        self.terminated = true;
        Ok(())
    }

    /// Ends a sequential block with a dispatcher return.
    pub fn terminate_dispatch(&mut self) -> Result<(), Mips4BlockBuildError> {
        if self.terminated {
            return Err(Mips4BlockBuildError::DuplicateTerminator);
        }
        self.terminated = true;
        Ok(())
    }

    /// Validates structural and entry-context invariants.
    pub fn verify(&self) -> Result<(), Mips4BlockBuildError> {
        if self.instruction_count() == 0 {
            return Err(Mips4BlockBuildError::Empty);
        }
        if !self.terminated {
            return Err(Mips4BlockBuildError::MissingTerminator);
        }
        if self.branch.is_none() && self.delay_slot.is_some() {
            return Err(Mips4BlockBuildError::InvalidDelaySlot);
        }
        let first = self
            .body
            .first()
            .map(|instruction| instruction.metadata)
            .or_else(|| self.branch.map(|branch| branch.metadata))
            .ok_or(Mips4BlockBuildError::Empty)?;
        if first.pc != self.key.pc || first.delay_slot_branch_pc != self.key.delay_slot_branch_pc {
            return Err(Mips4BlockBuildError::EntryMismatch);
        }
        if self
            .body
            .iter()
            .any(|instruction| instruction.retire.pc != instruction.metadata.pc)
            || self
                .delay_slot
                .is_some_and(|instruction| instruction.retire.pc != instruction.metadata.pc)
            || self
                .branch
                .is_some_and(|branch| branch.retire.pc != branch.metadata.pc)
        {
            return Err(Mips4BlockBuildError::InvalidRetirement);
        }
        for window in self.body.windows(2) {
            if window[1].metadata.pc != window[0].metadata.pc.wrapping_add(4)
                || window[1].metadata.delay_slot_branch_pc.is_some()
            {
                return Err(Mips4BlockBuildError::NonSequentialBody);
            }
        }
        if let (Some(last), Some(branch)) = (self.body.last(), self.branch)
            && branch.metadata.pc != last.metadata.pc.wrapping_add(4)
        {
            return Err(Mips4BlockBuildError::NonSequentialBody);
        }
        Ok(())
    }

    fn check_page(&self, pc: u64) -> Result<(), Mips4BlockBuildError> {
        if pc & !0x0fff != self.key.pc & !0x0fff {
            Err(Mips4BlockBuildError::PageCrossing)
        } else {
            Ok(())
        }
    }
}

/// Invalid translated block construction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4BlockBuildError {
    /// A block contained no guest instruction.
    Empty,
    /// More than the configured instruction limit was requested.
    InstructionLimit,
    /// An instruction crossed the entry page.
    PageCrossing,
    /// An instruction followed an installed terminator.
    InstructionAfterTerminator,
    /// A second terminator was installed.
    DuplicateTerminator,
    /// The block did not explicitly return to the dispatcher or branch.
    MissingTerminator,
    /// An instruction retirement marker named the wrong guest PC.
    InvalidRetirement,
    /// Branch delay-slot metadata was inconsistent.
    InvalidDelaySlot,
    /// The first instruction did not match the block key.
    EntryMismatch,
    /// Sequential body addresses were inconsistent.
    NonSequentialBody,
    /// A Region contained an unsupported source, operation, or edge topology.
    InvalidRegion,
}

impl fmt::Display for Mips4BlockBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Empty => "MIPS IV block is empty",
            Self::InstructionLimit => "MIPS IV block exceeds its instruction limit",
            Self::PageCrossing => "MIPS IV block crosses a 4 KiB page",
            Self::InstructionAfterTerminator => {
                "MIPS IV block has an instruction after its terminator"
            }
            Self::DuplicateTerminator => "MIPS IV block has more than one terminator",
            Self::MissingTerminator => "MIPS IV block has no terminator",
            Self::InvalidRetirement => "MIPS IV block has invalid retirement metadata",
            Self::InvalidDelaySlot => "MIPS IV block has invalid delay-slot metadata",
            Self::EntryMismatch => "MIPS IV block entry does not match its key",
            Self::NonSequentialBody => "MIPS IV block body is not sequential",
            Self::InvalidRegion => "MIPS IV Region has an invalid execution topology",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for Mips4BlockBuildError {}

/// Stable exception subset emitted directly by block execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u64)]
pub enum Mips4BlockException {
    /// Signed arithmetic overflow.
    ArithmeticOverflow = 1,
    /// Misaligned instruction target.
    AddressErrorLoad = 2,
    /// Integer trap condition.
    Trap = 3,
    /// SYSCALL instruction.
    SystemCall = 4,
    /// BREAK instruction.
    Breakpoint = 5,
}

impl Mips4BlockException {
    /// Converts the stable block exception to the architecture exception.
    pub const fn architecture_exception(self) -> Mips4Exception {
        match self {
            Self::ArithmeticOverflow => Mips4Exception::ArithmeticOverflow,
            Self::AddressErrorLoad => Mips4Exception::AddressErrorLoad,
            Self::Trap => Mips4Exception::Trap,
            Self::SystemCall => Mips4Exception::Syscall,
            Self::Breakpoint => Mips4Exception::Breakpoint,
        }
    }

    /// Converts a stable integer code written by native code.
    pub const fn from_code(code: u64) -> Option<Self> {
        match code {
            1 => Some(Self::ArithmeticOverflow),
            2 => Some(Self::AddressErrorLoad),
            3 => Some(Self::Trap),
            4 => Some(Self::SystemCall),
            5 => Some(Self::Breakpoint),
            _ => None,
        }
    }
}

/// Stable exit code shared with native block functions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Mips4BlockExit {
    /// The caller-provided retirement budget was exhausted.
    BudgetExhausted = 1,
    /// The translated block completed and needs another dispatch.
    Dispatch = 2,
    /// An architectural exception was recorded in the frame.
    Exception = 3,
    /// The block guard became invalid before execution.
    GuardInvalid = 4,
    /// A typed runtime operation started an asynchronous transaction.
    RuntimeTransaction = 5,
    /// WAIT retired and left the processor in standby.
    RuntimeIdle = 6,
    /// The slice timeline cannot admit the next instruction.
    TimelineExhausted = 7,
    /// The block or native backend violated an internal invariant.
    InternalError = 8,
}

impl Mips4BlockExit {
    /// Converts a native return code to a typed exit.
    pub const fn from_code(code: u32) -> Option<Self> {
        match code {
            1 => Some(Self::BudgetExhausted),
            2 => Some(Self::Dispatch),
            3 => Some(Self::Exception),
            4 => Some(Self::GuardInvalid),
            5 => Some(Self::RuntimeTransaction),
            6 => Some(Self::RuntimeIdle),
            7 => Some(Self::TimelineExhausted),
            8 => Some(Self::InternalError),
            _ => None,
        }
    }
}

/// Stable, non-serialized hot state passed to interpreted and native blocks.
#[derive(Clone, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct Mips4BlockFrame {
    gpr: [u64; MIPS4_GPR_COUNT],
    gpr_write_through: *mut u64,
    hi: u64,
    lo: u64,
    pc: u64,
    next_pc: u64,
    delay_slot_branch_pc: u64,
    delay_slot_valid: u64,
    budget: u64,
    retired: u64,
    exception: u64,
    operations_executed: u64,
    runtime_calls: u64,
    operation_base: u64,
    runtime_call_base: u64,
    region_side_exit: u64,
    runtime_context: *mut (),
    runtime_call: usize,
    fast_memory_context: *mut (),
    fast_memory_native_context: *mut Mips4FastMemoryContext,
    fast_memory_read: usize,
    fast_memory_read_start: u64,
    fast_memory_read_end: u64,
    runtime_value: u64,
    runtime_memory_big_endian: u64,
    runtime_operation_values: *const Mips4RuntimeOperation,
    runtime_operations: *const Mips4RuntimeOperationDescriptor,
    runtime_operation_count: u64,
}

/// Byte offset of the GPR array in [`Mips4BlockFrame`].
pub const MIPS4_BLOCK_FRAME_GPR_OFFSET: i32 = core::mem::offset_of!(Mips4BlockFrame, gpr) as i32;
/// Byte offset of the active GPR write-through context in [`Mips4BlockFrame`].
pub const MIPS4_BLOCK_FRAME_GPR_WRITE_THROUGH_OFFSET: i32 =
    core::mem::offset_of!(Mips4BlockFrame, gpr_write_through) as i32;
/// Byte offset of HI in [`Mips4BlockFrame`].
pub const MIPS4_BLOCK_FRAME_HI_OFFSET: i32 = core::mem::offset_of!(Mips4BlockFrame, hi) as i32;
/// Byte offset of LO in [`Mips4BlockFrame`].
pub const MIPS4_BLOCK_FRAME_LO_OFFSET: i32 = core::mem::offset_of!(Mips4BlockFrame, lo) as i32;
/// Byte offset of PC in [`Mips4BlockFrame`].
pub const MIPS4_BLOCK_FRAME_PC_OFFSET: i32 = core::mem::offset_of!(Mips4BlockFrame, pc) as i32;
/// Byte offset of next PC in [`Mips4BlockFrame`].
pub const MIPS4_BLOCK_FRAME_NEXT_PC_OFFSET: i32 =
    core::mem::offset_of!(Mips4BlockFrame, next_pc) as i32;
/// Byte offset of the delay-slot branch PC in [`Mips4BlockFrame`].
pub const MIPS4_BLOCK_FRAME_DELAY_PC_OFFSET: i32 =
    core::mem::offset_of!(Mips4BlockFrame, delay_slot_branch_pc) as i32;
/// Byte offset of the delay-slot valid flag in [`Mips4BlockFrame`].
pub const MIPS4_BLOCK_FRAME_DELAY_VALID_OFFSET: i32 =
    core::mem::offset_of!(Mips4BlockFrame, delay_slot_valid) as i32;
/// Byte offset of the retirement budget in [`Mips4BlockFrame`].
pub const MIPS4_BLOCK_FRAME_BUDGET_OFFSET: i32 =
    core::mem::offset_of!(Mips4BlockFrame, budget) as i32;
/// Byte offset of the retired instruction count in [`Mips4BlockFrame`].
pub const MIPS4_BLOCK_FRAME_RETIRED_OFFSET: i32 =
    core::mem::offset_of!(Mips4BlockFrame, retired) as i32;
/// Byte offset of the exception code in [`Mips4BlockFrame`].
pub const MIPS4_BLOCK_FRAME_EXCEPTION_OFFSET: i32 =
    core::mem::offset_of!(Mips4BlockFrame, exception) as i32;
/// Byte offset of the entered-operation counter in [`Mips4BlockFrame`].
pub const MIPS4_BLOCK_FRAME_OPERATIONS_EXECUTED_OFFSET: i32 =
    core::mem::offset_of!(Mips4BlockFrame, operations_executed) as i32;
/// Byte offset of the entered runtime-helper counter in [`Mips4BlockFrame`].
pub const MIPS4_BLOCK_FRAME_RUNTIME_CALLS_OFFSET: i32 =
    core::mem::offset_of!(Mips4BlockFrame, runtime_calls) as i32;
/// Byte offset of the current Region operation-accounting base.
pub const MIPS4_BLOCK_FRAME_OPERATION_BASE_OFFSET: i32 =
    core::mem::offset_of!(Mips4BlockFrame, operation_base) as i32;
/// Byte offset of the current Region runtime-call accounting base.
pub const MIPS4_BLOCK_FRAME_RUNTIME_CALL_BASE_OFFSET: i32 =
    core::mem::offset_of!(Mips4BlockFrame, runtime_call_base) as i32;
/// Byte offset of the Region side-exit reason in [`Mips4BlockFrame`].
pub const MIPS4_BLOCK_FRAME_REGION_SIDE_EXIT_OFFSET: i32 =
    core::mem::offset_of!(Mips4BlockFrame, region_side_exit) as i32;
/// Byte offset of the opaque runtime context in [`Mips4BlockFrame`].
pub const MIPS4_BLOCK_FRAME_RUNTIME_CONTEXT_OFFSET: i32 =
    core::mem::offset_of!(Mips4BlockFrame, runtime_context) as i32;
/// Byte offset of the runtime trampoline address in [`Mips4BlockFrame`].
pub const MIPS4_BLOCK_FRAME_RUNTIME_CALL_OFFSET: i32 =
    core::mem::offset_of!(Mips4BlockFrame, runtime_call) as i32;
/// Byte offset of the machine-owned fast-memory context in [`Mips4BlockFrame`].
pub const MIPS4_BLOCK_FRAME_FAST_MEMORY_CONTEXT_OFFSET: i32 =
    core::mem::offset_of!(Mips4BlockFrame, fast_memory_context) as i32;
/// Byte offset of the stable native fast-memory view in [`Mips4BlockFrame`].
pub const MIPS4_BLOCK_FRAME_FAST_MEMORY_NATIVE_CONTEXT_OFFSET: i32 =
    core::mem::offset_of!(Mips4BlockFrame, fast_memory_native_context) as i32;
/// Byte offset of the ABI v3 fast-memory read entry in [`Mips4BlockFrame`].
pub const MIPS4_BLOCK_FRAME_FAST_MEMORY_READ_OFFSET: i32 =
    core::mem::offset_of!(Mips4BlockFrame, fast_memory_read) as i32;
/// Byte offset of the ABI v3 native-read physical range start.
pub const MIPS4_BLOCK_FRAME_FAST_MEMORY_READ_START_OFFSET: i32 =
    core::mem::offset_of!(Mips4BlockFrame, fast_memory_read_start) as i32;
/// Byte offset of the ABI v3 native-read physical range end.
pub const MIPS4_BLOCK_FRAME_FAST_MEMORY_READ_END_OFFSET: i32 =
    core::mem::offset_of!(Mips4BlockFrame, fast_memory_read_end) as i32;
/// Byte offset of the ABI v3 runtime result value in [`Mips4BlockFrame`].
pub const MIPS4_BLOCK_FRAME_RUNTIME_VALUE_OFFSET: i32 =
    core::mem::offset_of!(Mips4BlockFrame, runtime_value) as i32;
/// Byte offset of the runtime integer-memory endianness flag.
pub const MIPS4_BLOCK_FRAME_RUNTIME_MEMORY_BIG_ENDIAN_OFFSET: i32 =
    core::mem::offset_of!(Mips4BlockFrame, runtime_memory_big_endian) as i32;
/// Byte offset of the runtime operation descriptor table in [`Mips4BlockFrame`].
pub const MIPS4_BLOCK_FRAME_RUNTIME_OPERATIONS_OFFSET: i32 =
    core::mem::offset_of!(Mips4BlockFrame, runtime_operations) as i32;
/// Byte offset of the runtime operation descriptor count in [`Mips4BlockFrame`].
pub const MIPS4_BLOCK_FRAME_RUNTIME_OPERATION_COUNT_OFFSET: i32 =
    core::mem::offset_of!(Mips4BlockFrame, runtime_operation_count) as i32;

/// Returns the byte offset of one GPR in [`Mips4BlockFrame`].
pub const fn mips4_block_frame_gpr_offset(register: u8) -> i32 {
    MIPS4_BLOCK_FRAME_GPR_OFFSET + register as i32 * 8
}

impl Mips4BlockFrame {
    /// Creates a frame from explicit integer and control state.
    pub fn new(
        mut gpr: [u64; MIPS4_GPR_COUNT],
        hi: u64,
        lo: u64,
        pc: u64,
        next_pc: u64,
        delay_slot_branch_pc: Option<u64>,
        budget: u64,
    ) -> Self {
        gpr[0] = 0;
        Self {
            gpr,
            gpr_write_through: core::ptr::null_mut(),
            hi,
            lo,
            pc,
            next_pc,
            delay_slot_branch_pc: delay_slot_branch_pc.unwrap_or(0),
            delay_slot_valid: u64::from(delay_slot_branch_pc.is_some()),
            budget,
            retired: 0,
            exception: 0,
            operations_executed: 0,
            runtime_calls: 0,
            operation_base: 0,
            runtime_call_base: 0,
            region_side_exit: 0,
            runtime_context: core::ptr::null_mut(),
            runtime_call: 0,
            fast_memory_context: core::ptr::null_mut(),
            fast_memory_native_context: core::ptr::null_mut(),
            fast_memory_read: 0,
            fast_memory_read_start: 0,
            fast_memory_read_end: 0,
            runtime_value: 0,
            runtime_memory_big_endian: 0,
            runtime_operation_values: core::ptr::null(),
            runtime_operations: core::ptr::null(),
            runtime_operation_count: 0,
        }
    }

    /// Reads a guest GPR.
    pub const fn read_gpr(&self, register: u8) -> u64 {
        if register == 0 {
            0
        } else {
            self.gpr[register as usize]
        }
    }

    /// Writes a guest GPR while preserving GPR zero.
    pub fn write_gpr(&mut self, register: u8, value: u64) {
        if register != 0 {
            self.gpr[register as usize] = value;
            if !self.gpr_write_through.is_null() {
                // SAFETY: The execution target or native backend binds a live
                // 32-entry GPR array for exactly the duration of block execution.
                unsafe { self.gpr_write_through.add(register as usize).write(value) };
            }
        }
    }

    /// Installs an internal native-code write-through context when none exists.
    pub fn prepare_native_gpr_write_through(&mut self) -> bool {
        if self.gpr_write_through.is_null() {
            self.gpr_write_through = self.gpr.as_mut_ptr();
            true
        } else {
            false
        }
    }

    /// Releases an internal native-code write-through context.
    pub fn release_native_gpr_write_through(&mut self, installed: bool) {
        if installed {
            self.gpr_write_through = core::ptr::null_mut();
        }
    }

    pub(super) fn bind_gpr_write_through(&mut self, context: *mut u64) {
        debug_assert!(!context.is_null());
        self.gpr_write_through = context;
    }

    pub(super) const fn gpr_write_through(&self) -> *mut u64 {
        self.gpr_write_through
    }

    pub(super) fn clear_gpr_write_through(&mut self) {
        self.gpr_write_through = core::ptr::null_mut();
    }

    /// Returns all GPR values.
    pub const fn gpr(&self) -> &[u64; MIPS4_GPR_COUNT] {
        &self.gpr
    }

    /// Returns HI.
    pub const fn hi(&self) -> u64 {
        self.hi
    }

    /// Returns LO.
    pub const fn lo(&self) -> u64 {
        self.lo
    }

    /// Returns the current PC.
    pub const fn pc(&self) -> u64 {
        self.pc
    }

    /// Returns the queued next PC.
    pub const fn next_pc(&self) -> u64 {
        self.next_pc
    }

    /// Returns the branch owning the current delay slot.
    pub const fn delay_slot_branch_pc(&self) -> Option<u64> {
        if self.delay_slot_valid != 0 {
            Some(self.delay_slot_branch_pc)
        } else {
            None
        }
    }

    /// Returns the remaining retirement budget.
    pub const fn budget(&self) -> u64 {
        self.budget
    }

    /// Returns the number of normally retired instructions.
    pub const fn retired(&self) -> u64 {
        self.retired
    }

    /// Returns the number of guest operations entered by this invocation.
    pub const fn operations_executed(&self) -> u64 {
        self.operations_executed
    }

    /// Returns the number of typed runtime helpers entered by this invocation.
    pub const fn runtime_calls(&self) -> u64 {
        self.runtime_calls
    }

    /// Returns the most recent native Region side-exit reason.
    pub const fn region_side_exit(&self) -> Option<Mips4RegionSideExit> {
        Mips4RegionSideExit::from_code(self.region_side_exit)
    }

    /// Returns the recorded block exception.
    pub const fn exception(&self) -> Option<Mips4BlockException> {
        Mips4BlockException::from_code(self.exception)
    }

    /// Resets per-invocation budget and result fields.
    pub fn prepare(&mut self, budget: u64) {
        self.budget = budget;
        self.retired = 0;
        self.exception = 0;
        self.operations_executed = 0;
        self.runtime_calls = 0;
        self.operation_base = 0;
        self.runtime_call_base = 0;
        self.region_side_exit = 0;
        self.gpr[0] = 0;
    }

    /// Restricts the remaining retirement budget without increasing it.
    pub fn limit_budget(&mut self, budget: u64) {
        self.budget = self.budget.min(budget);
    }

    fn install_runtime<R>(
        &mut self,
        runtime: &mut R,
        operation_values: &[Mips4RuntimeOperation],
        operations: &[Mips4RuntimeOperationDescriptor],
    ) where
        R: Mips4BlockRuntime,
    {
        self.runtime_context = core::ptr::from_mut(runtime).cast();
        self.runtime_call = mips4_runtime_trampoline::<R> as *const () as usize;
        if let Some(abi) = runtime.runtime_abi_v3() {
            self.fast_memory_context = abi.context;
            self.fast_memory_native_context = abi.native_context;
            self.fast_memory_read = abi.frame_read as *const () as usize;
            self.fast_memory_read_start = abi.read_start;
            self.fast_memory_read_end = abi.read_end;
        } else {
            self.fast_memory_context = core::ptr::null_mut();
            self.fast_memory_native_context = core::ptr::null_mut();
            self.fast_memory_read = 0;
            self.fast_memory_read_start = 0;
            self.fast_memory_read_end = 0;
        }
        self.runtime_memory_big_endian = u64::from(runtime.runtime_memory_big_endian());
        self.runtime_operation_values = operation_values.as_ptr();
        self.runtime_operations = operations.as_ptr();
        self.runtime_operation_count = operations.len() as u64;
    }

    fn clear_runtime(&mut self) {
        self.runtime_context = core::ptr::null_mut();
        self.fast_memory_context = core::ptr::null_mut();
        self.fast_memory_native_context = core::ptr::null_mut();
        self.fast_memory_read = 0;
        self.fast_memory_read_start = 0;
        self.fast_memory_read_end = 0;
    }

    pub(super) fn replace_control(
        &mut self,
        pc: u64,
        next_pc: u64,
        delay_slot_branch_pc: Option<u64>,
    ) {
        self.pc = pc;
        self.next_pc = next_pc;
        self.delay_slot_branch_pc = delay_slot_branch_pc.unwrap_or(0);
        self.delay_slot_valid = u64::from(delay_slot_branch_pc.is_some());
    }
}

extern "C" fn mips4_runtime_trampoline<R>(
    context: *mut (),
    frame: *mut Mips4BlockFrame,
    operation: u32,
) -> u32
where
    R: Mips4BlockRuntime,
{
    let result = catch_unwind(AssertUnwindSafe(|| {
        if context.is_null() || frame.is_null() {
            return Mips4RuntimeResult::InternalError;
        }
        // SAFETY: The engine installs pointers to a live runtime and operation
        // table while uniquely borrowing the frame for native execution.
        let runtime = unsafe { &mut *context.cast::<R>() };
        let operations = unsafe {
            core::slice::from_raw_parts(
                (*frame).runtime_operation_values,
                (*frame).runtime_operation_count as usize,
            )
        };
        // SAFETY: The frame pointer is live and uniquely borrowed for the call.
        let frame = unsafe { &mut *frame };
        let Some(operation) = operations.get(operation as usize).copied() else {
            return Mips4RuntimeResult::InternalError;
        };
        runtime.execute(frame, operation)
    }));
    result.unwrap_or(Mips4RuntimeResult::InternalError) as u32
}

/// Lifts one decoded CPU instruction to typed block form.
pub fn lift_cpu_instruction(
    policy: &impl Mips4ExecutionPolicy,
    metadata: Mips4BlockInstructionMetadata,
    instruction: Mips4CpuInstruction,
) -> Mips4BlockLiftedInstruction {
    let raw = Mips4Instruction::from_bits(metadata.instruction);
    let signed = Mips4BlockOperand::SignedImmediate(raw.signed_immediate());
    let unsigned = Mips4BlockOperand::UnsignedImmediate(raw.immediate());
    let register = |value| Mips4BlockOperand::Register(value);
    let noop_on_invalid_word = matches!(
        policy.not_word_value_policy(instruction),
        Mips4NotWordValuePolicy::NoOperation
    );
    let sequential = |operation| {
        Mips4BlockLiftedInstruction::Sequential(Mips4BlockInstruction {
            metadata,
            operation,
            retire: Mips4BlockRetire { pc: metadata.pc },
        })
    };
    let arithmetic = |operation, width, trap_on_overflow, destination, lhs, rhs| {
        sequential(Mips4BlockOperation::Arithmetic {
            operation,
            width,
            trap_on_overflow,
            noop_on_invalid_word,
            destination,
            lhs,
            rhs,
        })
    };
    let logical = |operation, destination, lhs, rhs| {
        sequential(Mips4BlockOperation::Logical {
            operation,
            destination,
            lhs,
            rhs,
        })
    };
    let shift = |operation, width, destination, value, amount| {
        sequential(Mips4BlockOperation::Shift {
            operation,
            width,
            noop_on_invalid_word,
            destination,
            value,
            amount,
        })
    };
    let compare = |comparison, destination, lhs, rhs| {
        sequential(Mips4BlockOperation::Compare {
            comparison,
            destination,
            lhs,
            rhs,
        })
    };
    let branch = |condition, target, likely, link| {
        Mips4BlockLiftedInstruction::Branch(Mips4BlockBranch {
            metadata,
            condition,
            target,
            likely,
            link,
            retire: Mips4BlockRetire { pc: metadata.pc },
        })
    };
    let branch_target = || {
        metadata
            .pc
            .wrapping_add(4)
            .wrapping_add((raw.signed_immediate() as i64 as u64).wrapping_shl(2))
    };
    let jump_target =
        || (metadata.pc.wrapping_add(4) & !0x0fff_ffff) | (u64::from(raw.target()) << 2);

    match instruction {
        Mips4CpuInstruction::Add => arithmetic(
            Mips4BlockArithmetic::Add,
            Mips4BlockWidth::Word,
            true,
            raw.rd(),
            raw.rs(),
            register(raw.rt()),
        ),
        Mips4CpuInstruction::Addi => arithmetic(
            Mips4BlockArithmetic::Add,
            Mips4BlockWidth::Word,
            true,
            raw.rt(),
            raw.rs(),
            signed,
        ),
        Mips4CpuInstruction::Addiu => arithmetic(
            Mips4BlockArithmetic::Add,
            Mips4BlockWidth::Word,
            false,
            raw.rt(),
            raw.rs(),
            signed,
        ),
        Mips4CpuInstruction::Addu => arithmetic(
            Mips4BlockArithmetic::Add,
            Mips4BlockWidth::Word,
            false,
            raw.rd(),
            raw.rs(),
            register(raw.rt()),
        ),
        Mips4CpuInstruction::Sub => arithmetic(
            Mips4BlockArithmetic::Subtract,
            Mips4BlockWidth::Word,
            true,
            raw.rd(),
            raw.rs(),
            register(raw.rt()),
        ),
        Mips4CpuInstruction::Subu => arithmetic(
            Mips4BlockArithmetic::Subtract,
            Mips4BlockWidth::Word,
            false,
            raw.rd(),
            raw.rs(),
            register(raw.rt()),
        ),
        Mips4CpuInstruction::Dadd => arithmetic(
            Mips4BlockArithmetic::Add,
            Mips4BlockWidth::Doubleword,
            true,
            raw.rd(),
            raw.rs(),
            register(raw.rt()),
        ),
        Mips4CpuInstruction::Daddi => arithmetic(
            Mips4BlockArithmetic::Add,
            Mips4BlockWidth::Doubleword,
            true,
            raw.rt(),
            raw.rs(),
            signed,
        ),
        Mips4CpuInstruction::Daddiu => arithmetic(
            Mips4BlockArithmetic::Add,
            Mips4BlockWidth::Doubleword,
            false,
            raw.rt(),
            raw.rs(),
            signed,
        ),
        Mips4CpuInstruction::Daddu => arithmetic(
            Mips4BlockArithmetic::Add,
            Mips4BlockWidth::Doubleword,
            false,
            raw.rd(),
            raw.rs(),
            register(raw.rt()),
        ),
        Mips4CpuInstruction::Dsub => arithmetic(
            Mips4BlockArithmetic::Subtract,
            Mips4BlockWidth::Doubleword,
            true,
            raw.rd(),
            raw.rs(),
            register(raw.rt()),
        ),
        Mips4CpuInstruction::Dsubu => arithmetic(
            Mips4BlockArithmetic::Subtract,
            Mips4BlockWidth::Doubleword,
            false,
            raw.rd(),
            raw.rs(),
            register(raw.rt()),
        ),
        Mips4CpuInstruction::And => logical(
            Mips4BlockLogical::And,
            raw.rd(),
            raw.rs(),
            register(raw.rt()),
        ),
        Mips4CpuInstruction::Andi => logical(Mips4BlockLogical::And, raw.rt(), raw.rs(), unsigned),
        Mips4CpuInstruction::Or => logical(
            Mips4BlockLogical::Or,
            raw.rd(),
            raw.rs(),
            register(raw.rt()),
        ),
        Mips4CpuInstruction::Ori => logical(Mips4BlockLogical::Or, raw.rt(), raw.rs(), unsigned),
        Mips4CpuInstruction::Xor => logical(
            Mips4BlockLogical::Xor,
            raw.rd(),
            raw.rs(),
            register(raw.rt()),
        ),
        Mips4CpuInstruction::Xori => logical(Mips4BlockLogical::Xor, raw.rt(), raw.rs(), unsigned),
        Mips4CpuInstruction::Nor => logical(
            Mips4BlockLogical::Nor,
            raw.rd(),
            raw.rs(),
            register(raw.rt()),
        ),
        Mips4CpuInstruction::Lui => sequential(Mips4BlockOperation::LoadUpperImmediate {
            destination: raw.rt(),
            immediate: raw.immediate(),
        }),
        Mips4CpuInstruction::Sll => shift(
            Mips4BlockShift::Left,
            Mips4BlockWidth::Word,
            raw.rd(),
            raw.rt(),
            Mips4BlockShiftAmount::Immediate(raw.shamt()),
        ),
        Mips4CpuInstruction::Sllv => shift(
            Mips4BlockShift::Left,
            Mips4BlockWidth::Word,
            raw.rd(),
            raw.rt(),
            Mips4BlockShiftAmount::Register(raw.rs()),
        ),
        Mips4CpuInstruction::Srl => shift(
            Mips4BlockShift::RightLogical,
            Mips4BlockWidth::Word,
            raw.rd(),
            raw.rt(),
            Mips4BlockShiftAmount::Immediate(raw.shamt()),
        ),
        Mips4CpuInstruction::Srlv => shift(
            Mips4BlockShift::RightLogical,
            Mips4BlockWidth::Word,
            raw.rd(),
            raw.rt(),
            Mips4BlockShiftAmount::Register(raw.rs()),
        ),
        Mips4CpuInstruction::Sra => shift(
            Mips4BlockShift::RightArithmetic,
            Mips4BlockWidth::Word,
            raw.rd(),
            raw.rt(),
            Mips4BlockShiftAmount::Immediate(raw.shamt()),
        ),
        Mips4CpuInstruction::Srav => shift(
            Mips4BlockShift::RightArithmetic,
            Mips4BlockWidth::Word,
            raw.rd(),
            raw.rt(),
            Mips4BlockShiftAmount::Register(raw.rs()),
        ),
        Mips4CpuInstruction::Dsll | Mips4CpuInstruction::Dsll32 => shift(
            Mips4BlockShift::Left,
            Mips4BlockWidth::Doubleword,
            raw.rd(),
            raw.rt(),
            Mips4BlockShiftAmount::Immediate(
                raw.shamt() + u8::from(matches!(instruction, Mips4CpuInstruction::Dsll32)) * 32,
            ),
        ),
        Mips4CpuInstruction::Dsllv => shift(
            Mips4BlockShift::Left,
            Mips4BlockWidth::Doubleword,
            raw.rd(),
            raw.rt(),
            Mips4BlockShiftAmount::Register(raw.rs()),
        ),
        Mips4CpuInstruction::Dsrl | Mips4CpuInstruction::Dsrl32 => shift(
            Mips4BlockShift::RightLogical,
            Mips4BlockWidth::Doubleword,
            raw.rd(),
            raw.rt(),
            Mips4BlockShiftAmount::Immediate(
                raw.shamt() + u8::from(matches!(instruction, Mips4CpuInstruction::Dsrl32)) * 32,
            ),
        ),
        Mips4CpuInstruction::Dsrlv => shift(
            Mips4BlockShift::RightLogical,
            Mips4BlockWidth::Doubleword,
            raw.rd(),
            raw.rt(),
            Mips4BlockShiftAmount::Register(raw.rs()),
        ),
        Mips4CpuInstruction::Dsra | Mips4CpuInstruction::Dsra32 => shift(
            Mips4BlockShift::RightArithmetic,
            Mips4BlockWidth::Doubleword,
            raw.rd(),
            raw.rt(),
            Mips4BlockShiftAmount::Immediate(
                raw.shamt() + u8::from(matches!(instruction, Mips4CpuInstruction::Dsra32)) * 32,
            ),
        ),
        Mips4CpuInstruction::Dsrav => shift(
            Mips4BlockShift::RightArithmetic,
            Mips4BlockWidth::Doubleword,
            raw.rd(),
            raw.rt(),
            Mips4BlockShiftAmount::Register(raw.rs()),
        ),
        Mips4CpuInstruction::Slt => compare(
            Mips4BlockComparison::SignedLessThan,
            raw.rd(),
            raw.rs(),
            register(raw.rt()),
        ),
        Mips4CpuInstruction::Slti => compare(
            Mips4BlockComparison::SignedLessThan,
            raw.rt(),
            raw.rs(),
            signed,
        ),
        Mips4CpuInstruction::Sltu => compare(
            Mips4BlockComparison::UnsignedLessThan,
            raw.rd(),
            raw.rs(),
            register(raw.rt()),
        ),
        Mips4CpuInstruction::Sltiu => compare(
            Mips4BlockComparison::UnsignedLessThan,
            raw.rt(),
            raw.rs(),
            signed,
        ),
        Mips4CpuInstruction::Mult
        | Mips4CpuInstruction::Multu
        | Mips4CpuInstruction::Dmult
        | Mips4CpuInstruction::Dmultu => sequential(Mips4BlockOperation::Multiply {
            width: if matches!(
                instruction,
                Mips4CpuInstruction::Mult | Mips4CpuInstruction::Multu
            ) {
                Mips4BlockWidth::Word
            } else {
                Mips4BlockWidth::Doubleword
            },
            signed: matches!(
                instruction,
                Mips4CpuInstruction::Mult | Mips4CpuInstruction::Dmult
            ),
            noop_on_invalid_word,
            lhs: raw.rs(),
            rhs: raw.rt(),
        }),
        Mips4CpuInstruction::Div
        | Mips4CpuInstruction::Divu
        | Mips4CpuInstruction::Ddiv
        | Mips4CpuInstruction::Ddivu => sequential(Mips4BlockOperation::Divide {
            width: if matches!(
                instruction,
                Mips4CpuInstruction::Div | Mips4CpuInstruction::Divu
            ) {
                Mips4BlockWidth::Word
            } else {
                Mips4BlockWidth::Doubleword
            },
            signed: matches!(
                instruction,
                Mips4CpuInstruction::Div | Mips4CpuInstruction::Ddiv
            ),
            noop_on_invalid_word,
            lhs: raw.rs(),
            rhs: raw.rt(),
        }),
        Mips4CpuInstruction::Mfhi | Mips4CpuInstruction::Mflo => {
            sequential(Mips4BlockOperation::MoveFromSpecial {
                high: matches!(instruction, Mips4CpuInstruction::Mfhi),
                destination: raw.rd(),
            })
        }
        Mips4CpuInstruction::Mthi | Mips4CpuInstruction::Mtlo => {
            sequential(Mips4BlockOperation::MoveToSpecial {
                high: matches!(instruction, Mips4CpuInstruction::Mthi),
                source: raw.rs(),
            })
        }
        Mips4CpuInstruction::Movn | Mips4CpuInstruction::Movz => {
            sequential(Mips4BlockOperation::ConditionalMove {
                when_zero: matches!(instruction, Mips4CpuInstruction::Movz),
                destination: raw.rd(),
                source: raw.rs(),
                condition: raw.rt(),
            })
        }
        Mips4CpuInstruction::Teq
        | Mips4CpuInstruction::Teqi
        | Mips4CpuInstruction::Tge
        | Mips4CpuInstruction::Tgei
        | Mips4CpuInstruction::Tgeiu
        | Mips4CpuInstruction::Tgeu
        | Mips4CpuInstruction::Tlt
        | Mips4CpuInstruction::Tlti
        | Mips4CpuInstruction::Tltiu
        | Mips4CpuInstruction::Tltu
        | Mips4CpuInstruction::Tne
        | Mips4CpuInstruction::Tnei => {
            let (trap, rhs) = match instruction {
                Mips4CpuInstruction::Teq => (Mips4BlockTrap::Equal, register(raw.rt())),
                Mips4CpuInstruction::Teqi => (Mips4BlockTrap::Equal, signed),
                Mips4CpuInstruction::Tne => (Mips4BlockTrap::NotEqual, register(raw.rt())),
                Mips4CpuInstruction::Tnei => (Mips4BlockTrap::NotEqual, signed),
                Mips4CpuInstruction::Tge => {
                    (Mips4BlockTrap::SignedGreaterThanOrEqual, register(raw.rt()))
                }
                Mips4CpuInstruction::Tgei => (Mips4BlockTrap::SignedGreaterThanOrEqual, signed),
                Mips4CpuInstruction::Tgeu => (
                    Mips4BlockTrap::UnsignedGreaterThanOrEqual,
                    register(raw.rt()),
                ),
                Mips4CpuInstruction::Tgeiu => (Mips4BlockTrap::UnsignedGreaterThanOrEqual, signed),
                Mips4CpuInstruction::Tlt => (Mips4BlockTrap::SignedLessThan, register(raw.rt())),
                Mips4CpuInstruction::Tlti => (Mips4BlockTrap::SignedLessThan, signed),
                Mips4CpuInstruction::Tltu => (Mips4BlockTrap::UnsignedLessThan, register(raw.rt())),
                Mips4CpuInstruction::Tltiu => (Mips4BlockTrap::UnsignedLessThan, signed),
                _ => unreachable!(),
            };
            sequential(Mips4BlockOperation::Trap {
                trap,
                lhs: raw.rs(),
                rhs,
            })
        }
        Mips4CpuInstruction::Syscall => sequential(Mips4BlockOperation::Exception(
            Mips4BlockException::SystemCall,
        )),
        Mips4CpuInstruction::Break => sequential(Mips4BlockOperation::Exception(
            Mips4BlockException::Breakpoint,
        )),
        Mips4CpuInstruction::Sync => sequential(Mips4BlockOperation::NoOperation),
        Mips4CpuInstruction::Pref => sequential(Mips4BlockOperation::Runtime(
            Mips4RuntimeOperation::Prefetch { raw },
        )),
        Mips4CpuInstruction::Beq | Mips4CpuInstruction::Beql => branch(
            Mips4BlockBranchCondition::Equal {
                lhs: raw.rs(),
                rhs: raw.rt(),
            },
            Mips4BlockBranchTarget::Direct(branch_target()),
            matches!(instruction, Mips4CpuInstruction::Beql),
            None,
        ),
        Mips4CpuInstruction::Bne | Mips4CpuInstruction::Bnel => branch(
            Mips4BlockBranchCondition::NotEqual {
                lhs: raw.rs(),
                rhs: raw.rt(),
            },
            Mips4BlockBranchTarget::Direct(branch_target()),
            matches!(instruction, Mips4CpuInstruction::Bnel),
            None,
        ),
        Mips4CpuInstruction::Bltz
        | Mips4CpuInstruction::Bltzl
        | Mips4CpuInstruction::Bltzal
        | Mips4CpuInstruction::Bltzall => branch(
            Mips4BlockBranchCondition::LessThanZero { source: raw.rs() },
            Mips4BlockBranchTarget::Direct(branch_target()),
            matches!(
                instruction,
                Mips4CpuInstruction::Bltzl | Mips4CpuInstruction::Bltzall
            ),
            matches!(
                instruction,
                Mips4CpuInstruction::Bltzal | Mips4CpuInstruction::Bltzall
            )
            .then_some(31),
        ),
        Mips4CpuInstruction::Bgez
        | Mips4CpuInstruction::Bgezl
        | Mips4CpuInstruction::Bgezal
        | Mips4CpuInstruction::Bgezall => branch(
            Mips4BlockBranchCondition::GreaterThanOrEqualZero { source: raw.rs() },
            Mips4BlockBranchTarget::Direct(branch_target()),
            matches!(
                instruction,
                Mips4CpuInstruction::Bgezl | Mips4CpuInstruction::Bgezall
            ),
            matches!(
                instruction,
                Mips4CpuInstruction::Bgezal | Mips4CpuInstruction::Bgezall
            )
            .then_some(31),
        ),
        Mips4CpuInstruction::Blez | Mips4CpuInstruction::Blezl => branch(
            Mips4BlockBranchCondition::LessThanOrEqualZero { source: raw.rs() },
            Mips4BlockBranchTarget::Direct(branch_target()),
            matches!(instruction, Mips4CpuInstruction::Blezl),
            None,
        ),
        Mips4CpuInstruction::Bgtz | Mips4CpuInstruction::Bgtzl => branch(
            Mips4BlockBranchCondition::GreaterThanZero { source: raw.rs() },
            Mips4BlockBranchTarget::Direct(branch_target()),
            matches!(instruction, Mips4CpuInstruction::Bgtzl),
            None,
        ),
        Mips4CpuInstruction::J => branch(
            Mips4BlockBranchCondition::Always,
            Mips4BlockBranchTarget::Direct(jump_target()),
            false,
            None,
        ),
        Mips4CpuInstruction::Jal => branch(
            Mips4BlockBranchCondition::Always,
            Mips4BlockBranchTarget::Direct(jump_target()),
            false,
            Some(31),
        ),
        Mips4CpuInstruction::Jr => branch(
            Mips4BlockBranchCondition::Always,
            Mips4BlockBranchTarget::Register(raw.rs()),
            false,
            None,
        ),
        Mips4CpuInstruction::Jalr => branch(
            Mips4BlockBranchCondition::Always,
            Mips4BlockBranchTarget::Register(raw.rs()),
            false,
            Some(raw.rd()),
        ),
        Mips4CpuInstruction::Lb
        | Mips4CpuInstruction::Lbu
        | Mips4CpuInstruction::Ld
        | Mips4CpuInstruction::Ldl
        | Mips4CpuInstruction::Ldr
        | Mips4CpuInstruction::Lh
        | Mips4CpuInstruction::Lhu
        | Mips4CpuInstruction::Ll
        | Mips4CpuInstruction::Lld
        | Mips4CpuInstruction::Lw
        | Mips4CpuInstruction::Lwl
        | Mips4CpuInstruction::Lwr
        | Mips4CpuInstruction::Lwu
        | Mips4CpuInstruction::Sb
        | Mips4CpuInstruction::Sc
        | Mips4CpuInstruction::Scd
        | Mips4CpuInstruction::Sd
        | Mips4CpuInstruction::Sdl
        | Mips4CpuInstruction::Sdr
        | Mips4CpuInstruction::Sh
        | Mips4CpuInstruction::Sw
        | Mips4CpuInstruction::Swl
        | Mips4CpuInstruction::Swr => sequential(Mips4BlockOperation::Runtime(
            Mips4RuntimeOperation::Memory { instruction, raw },
        )),
    }
}

/// Executes a runtime-free typed block using the portable reference interpreter.
pub fn interpret_block(block: &Mips4Block, frame: &mut Mips4BlockFrame) -> Mips4BlockExit {
    struct RejectRuntime;

    impl Mips4BlockRuntime for RejectRuntime {
        fn execute(
            &mut self,
            _frame: &mut Mips4BlockFrame,
            _operation: Mips4RuntimeOperation,
        ) -> Mips4RuntimeResult {
            Mips4RuntimeResult::InternalError
        }
    }

    interpret_block_with_runtime(block, frame, &mut RejectRuntime)
}

/// Executes a typed block with shared runtime semantics.
pub fn interpret_block_with_runtime<R>(
    block: &Mips4Block,
    frame: &mut Mips4BlockFrame,
    runtime: &mut R,
) -> Mips4BlockExit
where
    R: Mips4BlockRuntime + ?Sized,
{
    if block.verify().is_err() || frame.pc != block.key.pc || frame.budget == 0 {
        return Mips4BlockExit::InternalError;
    }
    frame.operations_executed = 0;

    for instruction in &block.body {
        if frame.pc != instruction.metadata.pc {
            return Mips4BlockExit::InternalError;
        }
        frame.operations_executed = frame.operations_executed.saturating_add(1);
        match interpret_operation(instruction.operation, frame, runtime) {
            Ok(Mips4RuntimeResult::Continue) => {}
            Ok(Mips4RuntimeResult::ContinueControl) => {
                if retire(frame) {
                    return Mips4BlockExit::BudgetExhausted;
                }
                continue;
            }
            Ok(Mips4RuntimeResult::DispatchSequential) => {
                retire_sequential(frame);
                return if retire(frame) {
                    Mips4BlockExit::BudgetExhausted
                } else {
                    Mips4BlockExit::Dispatch
                };
            }
            Ok(Mips4RuntimeResult::DispatchControl) => {
                return if retire(frame) {
                    Mips4BlockExit::BudgetExhausted
                } else {
                    Mips4BlockExit::Dispatch
                };
            }
            Ok(Mips4RuntimeResult::Transaction) => {
                return Mips4BlockExit::RuntimeTransaction;
            }
            Ok(Mips4RuntimeResult::Exception) => return Mips4BlockExit::Exception,
            Ok(Mips4RuntimeResult::Idle) => {
                let _ = retire(frame);
                return Mips4BlockExit::RuntimeIdle;
            }
            Ok(Mips4RuntimeResult::TimelineExhausted) => {
                frame.operations_executed = frame.operations_executed.saturating_sub(1);
                return Mips4BlockExit::TimelineExhausted;
            }
            Ok(Mips4RuntimeResult::InternalError) => return Mips4BlockExit::InternalError,
            Err(exception) => {
                frame.exception = exception as u64;
                return Mips4BlockExit::Exception;
            }
        }
        retire_sequential(frame);
        if retire(frame) {
            return Mips4BlockExit::BudgetExhausted;
        }
    }

    let Some(branch) = block.branch else {
        return Mips4BlockExit::Dispatch;
    };
    if frame.pc != branch.metadata.pc {
        return Mips4BlockExit::InternalError;
    }
    frame.operations_executed = frame.operations_executed.saturating_add(1);
    let target = match branch.target {
        Mips4BlockBranchTarget::Direct(target) => target,
        Mips4BlockBranchTarget::Register(register) => {
            let target = frame.read_gpr(register);
            if target & 0x3 != 0 {
                frame.exception = Mips4BlockException::AddressErrorLoad as u64;
                return Mips4BlockExit::Exception;
            }
            target
        }
    };
    if let Some(link) = branch.link {
        frame.write_gpr(link, branch.metadata.pc.wrapping_add(8));
    }
    let taken = branch_condition(branch.condition, frame);
    retire_branch(frame, branch.metadata.pc, target, taken, branch.likely);
    if retire(frame) {
        return Mips4BlockExit::BudgetExhausted;
    }
    if !taken && branch.likely {
        return Mips4BlockExit::Dispatch;
    }

    let Some(delay_slot) = block.delay_slot else {
        return Mips4BlockExit::Dispatch;
    };
    if frame.pc != delay_slot.metadata.pc {
        return Mips4BlockExit::InternalError;
    }
    frame.operations_executed = frame.operations_executed.saturating_add(1);
    match interpret_operation(delay_slot.operation, frame, runtime) {
        Ok(Mips4RuntimeResult::Continue) => {}
        Ok(Mips4RuntimeResult::ContinueControl) => {
            return if retire(frame) {
                Mips4BlockExit::BudgetExhausted
            } else {
                Mips4BlockExit::Dispatch
            };
        }
        Ok(Mips4RuntimeResult::DispatchSequential) => {
            retire_sequential(frame);
            return if retire(frame) {
                Mips4BlockExit::BudgetExhausted
            } else {
                Mips4BlockExit::Dispatch
            };
        }
        Ok(Mips4RuntimeResult::DispatchControl) => {
            return if retire(frame) {
                Mips4BlockExit::BudgetExhausted
            } else {
                Mips4BlockExit::Dispatch
            };
        }
        Ok(Mips4RuntimeResult::Transaction) => return Mips4BlockExit::RuntimeTransaction,
        Ok(Mips4RuntimeResult::Exception) => return Mips4BlockExit::Exception,
        Ok(Mips4RuntimeResult::Idle) => {
            let _ = retire(frame);
            return Mips4BlockExit::RuntimeIdle;
        }
        Ok(Mips4RuntimeResult::TimelineExhausted) => {
            frame.operations_executed = frame.operations_executed.saturating_sub(1);
            return Mips4BlockExit::TimelineExhausted;
        }
        Ok(Mips4RuntimeResult::InternalError) => return Mips4BlockExit::InternalError,
        Err(exception) => {
            frame.exception = exception as u64;
            return Mips4BlockExit::Exception;
        }
    }
    retire_sequential(frame);
    if retire(frame) {
        Mips4BlockExit::BudgetExhausted
    } else {
        Mips4BlockExit::Dispatch
    }
}

/// Host-code backend used by the tiered block engine.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Mips4RegionKey {
    /// Region entry block identity.
    pub entry: Mips4BlockKey,
}

/// One unique basic-block node owned by a Region.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mips4RegionNode {
    block: Mips4Block,
    hot_successor: Option<usize>,
}

impl Mips4RegionNode {
    /// Creates one Region node with an optional in-Region hot successor.
    pub fn new(block: Mips4Block, hot_successor: Option<usize>) -> Self {
        Self {
            block,
            hot_successor,
        }
    }

    /// Returns the verified block represented by this node.
    pub const fn block(&self) -> &Mips4Block {
        &self.block
    }

    /// Returns the hot in-Region successor node.
    pub const fn hot_successor(&self) -> Option<usize> {
        self.hot_successor
    }
}

/// Verified bounded control-flow Region compiled as one native function.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mips4Region {
    key: Mips4RegionKey,
    nodes: Vec<Mips4RegionNode>,
}

impl Mips4Region {
    /// Creates and verifies a bounded Region from unique block nodes.
    pub fn new(nodes: Vec<Mips4RegionNode>) -> Result<Self, Mips4BlockBuildError> {
        let entry = nodes
            .first()
            .ok_or(Mips4BlockBuildError::InvalidRegion)?
            .block
            .key();
        let region = Self {
            key: Mips4RegionKey { entry },
            nodes,
        };
        region.verify()?;
        Ok(region)
    }

    fn runtime_operations(&self) -> Vec<Mips4RuntimeOperation> {
        self.nodes
            .iter()
            .flat_map(|node| node.block.runtime_operations())
            .collect()
    }

    fn member_keys(&self) -> Vec<Mips4BlockKey> {
        self.nodes.iter().map(|node| node.block.key()).collect()
    }

    fn guards(&self) -> Vec<Mips4BlockGuard> {
        self.nodes
            .iter()
            .map(|node| node.block.guard().clone())
            .collect()
    }

    fn has_internal_edge(&self) -> bool {
        self.nodes.iter().any(|node| node.hot_successor.is_some())
    }

    fn has_cycle(&self) -> bool {
        self.nodes.iter().enumerate().any(|(index, node)| {
            node.hot_successor
                .is_some_and(|successor| successor <= index)
        })
    }

    fn contains_counter_barrier(&self) -> bool {
        self.runtime_operations()
            .iter()
            .any(|operation| matches!(operation, Mips4RuntimeOperation::Cp0 { .. }))
    }

    fn contains_guard_mutation(&self) -> bool {
        self.runtime_operations()
            .iter()
            .any(|operation| matches!(operation, Mips4RuntimeOperation::Cache { .. }))
    }

    fn operation_count(&self) -> usize {
        self.nodes
            .iter()
            .map(|node| node.block.instruction_count())
            .sum()
    }

    fn source_kind(&self) -> Option<Mips4RegionSource> {
        region_source_kind(self.nodes.first()?.block.guard())
    }

    fn is_executable(&self) -> bool {
        self.has_internal_edge()
            && (self.has_cycle() || self.operation_count() >= MIPS4_REGION_MIN_ACYCLIC_OPERATIONS)
            && !self.contains_counter_barrier()
            && !self.contains_guard_mutation()
            && self.operation_count() <= MIPS4_REGION_MAX_OPERATIONS
            && self.source_kind().is_some()
            && self.nodes.iter().all(|node| {
                node.block.runtime_operations().iter().all(|operation| {
                    !matches!(
                        operation,
                        Mips4RuntimeOperation::Cp0 { .. }
                            | Mips4RuntimeOperation::Cache { .. }
                            | Mips4RuntimeOperation::Coprocessor { .. }
                            | Mips4RuntimeOperation::Raise(_)
                            | Mips4RuntimeOperation::Cp1 {
                                decoded: Mips4Cp1Decode::Instruction(
                                    Mips4Cp1InstructionClass::Branch(_)
                                ),
                                ..
                            }
                    )
                })
            })
    }

    fn build_error(&self) -> Result<(), Mips4BlockBuildError> {
        if self.is_executable() {
            Ok(())
        } else {
            Err(Mips4BlockBuildError::InvalidRegion)
        }
    }

    /// Returns the Region entry identity.
    pub const fn key(&self) -> Mips4RegionKey {
        self.key
    }

    /// Returns unique Region nodes in lowering order.
    pub fn nodes(&self) -> &[Mips4RegionNode] {
        &self.nodes
    }

    /// Verifies Region bounds, contexts, blocks, and successor indices.
    pub fn verify(&self) -> Result<(), Mips4BlockBuildError> {
        if self.nodes.is_empty() || self.nodes.len() > MIPS4_REGION_MAX_NODES {
            return Err(Mips4BlockBuildError::InstructionLimit);
        }
        let mut operations = 0_usize;
        for node in &self.nodes {
            node.block.verify()?;
            operations = operations.saturating_add(node.block.instruction_count());
            if operations > MIPS4_REGION_MAX_OPERATIONS
                || node
                    .hot_successor
                    .is_some_and(|successor| successor >= self.nodes.len())
            {
                return Err(Mips4BlockBuildError::InstructionLimit);
            }
            let key = node.block.key();
            if key.fetch_context != self.key.entry.fetch_context
                || key.translation_generation != self.key.entry.translation_generation
                || key.code_guard != self.key.entry.code_guard
            {
                return Err(Mips4BlockBuildError::PageCrossing);
            }
        }
        if self.nodes[0].block.key() != self.key.entry {
            return Err(Mips4BlockBuildError::EntryMismatch);
        }
        let source_kind = self
            .source_kind()
            .ok_or(Mips4BlockBuildError::InvalidRegion)?;
        if self
            .nodes
            .iter()
            .any(|node| region_source_kind(node.block.guard()) != Some(source_kind))
        {
            return Err(Mips4BlockBuildError::InvalidRegion);
        }
        self.build_error()
    }
}

/// Reason a native Region returned to the Rust dispatcher.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Mips4RegionSideExit {
    /// The selected successor is outside the Region.
    ColdSuccessor = 1,
    /// The remaining budget cannot admit another node.
    Budget = 2,
    /// A typed runtime helper left native execution.
    Runtime = 3,
    /// A visibility guard or execution epoch changed.
    Guard = 4,
}

impl Mips4RegionSideExit {
    /// Converts a native ABI code to a typed Region side exit.
    pub const fn from_code(code: u64) -> Option<Self> {
        match code {
            1 => Some(Self::ColdSuccessor),
            2 => Some(Self::Budget),
            3 => Some(Self::Runtime),
            4 => Some(Self::Guard),
            _ => None,
        }
    }
}

/// Host-code backend used by the tiered block engine.
pub trait Mips4CodegenBackend {
    /// Backend-owned compiled block handle.
    type CompiledBlock;

    /// Backend-owned compiled Region handle.
    type CompiledRegion;

    /// Backend compilation or execution failure.
    type Error;

    /// Compiles one verified domain block.
    fn compile(&mut self, block: &Mips4Block) -> Result<Self::CompiledBlock, Self::Error>;

    /// Executes one compiled block against the stable frame ABI.
    fn execute(
        &mut self,
        compiled: &Self::CompiledBlock,
        frame: &mut Mips4BlockFrame,
    ) -> Result<Mips4BlockExit, Self::Error>;

    /// Compiles one verified bounded Region.
    fn compile_region(&mut self, region: &Mips4Region)
    -> Result<Self::CompiledRegion, Self::Error>;

    /// Executes one compiled Region against the stable frame ABI.
    fn execute_region(
        &mut self,
        compiled: &Self::CompiledRegion,
        frame: &mut Mips4BlockFrame,
    ) -> Result<Mips4BlockExit, Self::Error>;

    /// Drops all native code and resets backend allocation state.
    fn clear(&mut self) -> Result<(), Self::Error>;
}

/// Execution tier selected for one block entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4BlockTier {
    /// Portable domain-IR interpreter.
    Interpreter,

    /// Host-native backend.
    Native,

    /// Host-native bounded control-flow Region.
    Region,
}

/// Result of one tiered block invocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4BlockExecution {
    /// Architectural block exit.
    pub exit: Mips4BlockExit,

    /// Tier that executed the block.
    pub tier: Mips4BlockTier,

    /// Whether this block observes or changes dynamic CP0 counter state.
    pub counter_barrier: bool,

    /// Guest operations entered by this block invocation.
    pub operations_executed: u64,

    /// Typed runtime helpers entered by this block invocation.
    pub runtime_calls: u64,

    /// Native Region side-exit reason, when the Region tier executed.
    pub region_side_exit: Option<Mips4RegionSideExit>,
}

/// Result of probing and optionally executing one reusable I-cache block.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4CachedBlockExecution {
    /// No valid reusable block exists for the requested key.
    Missing,

    /// Deferred Count and Random updates must be committed before this block.
    CounterSynchronization,

    /// A reusable block executed normally.
    Executed(Mips4BlockExecution),
}

/// Derived block-engine counters.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Mips4BlockEngineStatistics {
    /// Blocks executed by the IR interpreter.
    pub interpreted_blocks: u64,

    /// Blocks executed as host-native code.
    pub native_blocks: u64,

    /// Guest operations entered by the IR interpreter.
    pub interpreted_operations: u64,

    /// Guest operations entered by host-native code.
    pub native_operations: u64,

    /// Typed runtime helper calls made by either tier.
    pub runtime_calls: u64,

    /// Dynamically fetched instructions translated as single-instruction blocks.
    pub dynamic_fetches: u64,

    /// Instructions fetched through a stable external code window.
    pub fast_fetches: u64,

    /// Blocks compiled by the native backend.
    pub compiled_blocks: u64,

    /// Cached blocks removed by guard invalidation.
    pub invalidated_blocks: u64,

    /// Whole-cache resets caused by the capacity limit or explicit reset.
    pub cache_resets: u64,

    /// Native Region function entries.
    pub region_entries: u64,

    /// Guest operations entered by native Regions.
    pub region_operations: u64,

    /// Regions compiled since the latest engine reset.
    pub region_compilations: u64,

    /// Region exits caused by an uncompiled successor edge.
    pub region_cold_side_exits: u64,

    /// Region exits caused by the retirement budget.
    pub region_budget_side_exits: u64,

    /// Region exits caused by a typed runtime operation.
    pub region_runtime_side_exits: u64,

    /// Region entries rejected by a visibility or execution guard.
    pub region_guard_side_exits: u64,

    /// Stable instructions fetched from System Flash.
    pub system_flash_fetches: u64,

    /// Stable instructions fetched from SDRAM.
    pub sdram_fetches: u64,
}

fn record_region_side_exit(
    statistics: &mut Mips4BlockEngineStatistics,
    side_exit: Option<Mips4RegionSideExit>,
) {
    match side_exit {
        Some(Mips4RegionSideExit::ColdSuccessor) => {
            statistics.region_cold_side_exits = statistics.region_cold_side_exits.saturating_add(1);
        }
        Some(Mips4RegionSideExit::Budget) => {
            statistics.region_budget_side_exits =
                statistics.region_budget_side_exits.saturating_add(1);
        }
        Some(Mips4RegionSideExit::Runtime) => {
            statistics.region_runtime_side_exits =
                statistics.region_runtime_side_exits.saturating_add(1);
        }
        Some(Mips4RegionSideExit::Guard) => {
            statistics.region_guard_side_exits =
                statistics.region_guard_side_exits.saturating_add(1);
        }
        None => {}
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Mips4SuccessorProfile {
    key: Option<Mips4BlockKey>,
    observations: u64,
}

fn record_successor<C, R>(record: &mut Mips4BlockRecord<C, R>, successor: Mips4BlockKey) {
    if let Some(profile) = record
        .successors
        .iter_mut()
        .find(|profile| profile.key == Some(successor))
    {
        profile.observations = profile.observations.saturating_add(1);
        return;
    }
    if let Some(profile) = record
        .successors
        .iter_mut()
        .find(|profile| profile.key.is_none())
    {
        *profile = Mips4SuccessorProfile {
            key: Some(successor),
            observations: 1,
        };
        return;
    }
    let profile = record
        .successors
        .iter_mut()
        .min_by_key(|profile| profile.observations)
        .expect("a fixed successor profile has entries");
    *profile = Mips4SuccessorProfile {
        key: Some(successor),
        observations: 1,
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mips4RegionSource {
    InstructionCache,
    Stable(Mips4CodeGuardKind),
}

fn region_source_kind(guard: &Mips4BlockGuard) -> Option<Mips4RegionSource> {
    match (guard.lines().is_empty(), guard.code_source()) {
        (false, None) => Some(Mips4RegionSource::InstructionCache),
        (true, Some(source)) => Some(Mips4RegionSource::Stable(source.kind)),
        _ => None,
    }
}

fn region_record_executable<C, R>(record: &Mips4BlockRecord<C, R>) -> bool {
    !record.counter_barrier
        && !record.guard_mutating
        && region_source_kind(record.block.guard()).is_some()
        && record.block.instruction_count() <= MIPS4_REGION_MAX_OPERATIONS
        && record.runtime_operations.iter().all(|operation| {
            !matches!(
                operation,
                Mips4RuntimeOperation::Cp0 { .. }
                    | Mips4RuntimeOperation::Cache { .. }
                    | Mips4RuntimeOperation::Coprocessor { .. }
                    | Mips4RuntimeOperation::Raise(_)
                    | Mips4RuntimeOperation::Cp1 {
                        decoded: Mips4Cp1Decode::Instruction(Mips4Cp1InstructionClass::Branch(_)),
                        ..
                    }
            )
        })
}

fn dominant_successor<C, R>(record: &Mips4BlockRecord<C, R>) -> Option<Mips4BlockKey> {
    let total = record
        .successors
        .iter()
        .map(|profile| profile.observations)
        .sum::<u64>();
    let dominant = record
        .successors
        .iter()
        .filter(|profile| profile.key.is_some())
        .max_by_key(|profile| profile.observations)?;
    let required_percent = if record
        .block
        .branch()
        .is_some_and(|branch| matches!(branch.target, Mips4BlockBranchTarget::Register(_)))
    {
        MIPS4_REGION_DOMINANT_INDIRECT_PERCENT
    } else {
        MIPS4_REGION_DOMINANT_DIRECT_PERCENT
    };
    (dominant.observations >= MIPS4_REGION_MIN_SUCCESSOR_OBSERVATIONS
        && dominant.observations.saturating_mul(100) >= total.saturating_mul(required_percent))
    .then_some(dominant.key?)
}

fn build_profiled_region<C, R>(
    entry_index: usize,
    indices: &Mips4BlockIndexMap,
    records: &[Option<Mips4BlockRecord<C, R>>],
) -> Option<Mips4Region> {
    let entry_record = records.get(entry_index)?.as_ref()?;
    if entry_record.compiled_region.is_some()
        || entry_record.region_hotness < MIPS4_REGION_HOT_THRESHOLD
        || !region_record_executable(entry_record)
    {
        return None;
    }
    let entry_key = entry_record.block.key();
    let entry_source = region_source_kind(entry_record.block.guard())?;
    let mut nodes = Vec::new();
    let mut node_keys = Vec::new();
    let mut operation_count = 0_usize;
    let mut current_index = entry_index;

    loop {
        let record = records.get(current_index)?.as_ref()?;
        let key = record.block.key();
        if nodes.len() == MIPS4_REGION_MAX_NODES
            || !region_record_executable(record)
            || key.fetch_context != entry_key.fetch_context
            || key.translation_generation != entry_key.translation_generation
            || key.code_guard != entry_key.code_guard
            || region_source_kind(record.block.guard()) != Some(entry_source)
        {
            break;
        }
        let next_operation_count = operation_count.checked_add(record.block.instruction_count())?;
        if next_operation_count > MIPS4_REGION_MAX_OPERATIONS {
            break;
        }
        operation_count = next_operation_count;
        let node_index = nodes.len();
        node_keys.push(key);
        nodes.push(Mips4RegionNode {
            block: record.block.clone(),
            hot_successor: None,
        });

        let Some(successor) = dominant_successor(record) else {
            break;
        };
        if let Some(successor_node) = node_keys.iter().position(|key| *key == successor) {
            nodes[node_index].hot_successor = Some(successor_node);
            break;
        }
        let Some(successor_index) = indices.get(&successor).copied() else {
            break;
        };
        let Some(successor_record) = records.get(successor_index).and_then(Option::as_ref) else {
            break;
        };
        let successor_key = successor_record.block.key();
        let successor_operations = successor_record.block.instruction_count();
        if nodes.len() == MIPS4_REGION_MAX_NODES
            || !region_record_executable(successor_record)
            || successor_key.fetch_context != entry_key.fetch_context
            || successor_key.translation_generation != entry_key.translation_generation
            || successor_key.code_guard != entry_key.code_guard
            || region_source_kind(successor_record.block.guard()) != Some(entry_source)
            || operation_count.saturating_add(successor_operations) > MIPS4_REGION_MAX_OPERATIONS
        {
            break;
        }
        nodes[node_index].hot_successor = Some(nodes.len());
        current_index = successor_index;
    }

    Mips4Region::new(nodes).ok()
}

struct Mips4CompiledRegionRecord<R> {
    compiled: R,
    runtime_operations: Vec<Mips4RuntimeOperation>,
    runtime_descriptors: Vec<Mips4RuntimeOperationDescriptor>,
    member_keys: Vec<Mips4BlockKey>,
    guards: Vec<Mips4BlockGuard>,
    guard_validation_epoch: Option<u64>,
}

struct Mips4BlockRecord<C, R> {
    block: Mips4Block,
    runtime_operations: Vec<Mips4RuntimeOperation>,
    runtime_descriptors: Vec<Mips4RuntimeOperationDescriptor>,
    counter_barrier: bool,
    guard_mutating: bool,
    guard_validation_epoch: Option<u64>,
    operation_hotness: u64,
    compiled: Option<C>,
    region_hotness: u64,
    region_next_compile_hotness: u64,
    successors: [Mips4SuccessorProfile; 4],
    compiled_region: Option<Mips4CompiledRegionRecord<R>>,
}

fn region_compile_due<C, R>(record: &Mips4BlockRecord<C, R>) -> bool {
    record.compiled_region.is_none() && record.region_hotness >= record.region_next_compile_hotness
}

fn compiled_region_guard_valid<R, T>(region: &mut Mips4CompiledRegionRecord<R>, runtime: &T) -> bool
where
    T: Mips4BlockRuntime,
{
    let epoch = runtime.block_guard_epoch();
    if region.guard_validation_epoch == Some(epoch) {
        return true;
    }
    let valid = region
        .guards
        .iter()
        .all(|guard| runtime.block_guard_valid(guard));
    if valid {
        region.guard_validation_epoch = Some(epoch);
    }
    valid
}

fn cached_guard_valid<C, G, R>(record: &mut Mips4BlockRecord<C, G>, runtime: &R) -> bool
where
    R: Mips4BlockRuntime,
{
    let epoch = runtime.block_guard_epoch();
    if record.guard_validation_epoch == Some(epoch) {
        return true;
    }
    let valid = runtime.block_guard_valid(record.block.guard());
    if valid {
        record.guard_validation_epoch = Some(epoch);
    }
    valid
}

#[cold]
#[inline(never)]
fn execute_interpreted_tier<B, R>(
    backend: &mut B,
    record: &mut Mips4BlockRecord<B::CompiledBlock, B::CompiledRegion>,
    frame: &mut Mips4BlockFrame,
    runtime: &mut R,
) -> Result<(Mips4BlockExit, bool), Mips4BlockEngineError<B::Error>>
where
    B: Mips4CodegenBackend,
    R: Mips4BlockRuntime,
{
    let exit = interpret_block_with_runtime(&record.block, frame, runtime);
    record.operation_hotness = record
        .operation_hotness
        .saturating_add(frame.operations_executed);
    let promoted = record.operation_hotness >= MIPS4_BLOCK_HOT_THRESHOLD;
    if promoted {
        record.compiled = Some(
            backend
                .compile(&record.block)
                .map_err(Mips4BlockEngineError::Backend)?,
        );
    }
    Ok((exit, promoted))
}

#[derive(Clone, Copy)]
struct Mips4BlockDispatchEntry {
    key: Mips4BlockKey,
    record: usize,
}

/// Non-serialized tiering, IR-cache, and native-code owner.
pub struct Mips4BlockEngine<B>
where
    B: Mips4CodegenBackend,
{
    backend: B,
    indices: Mips4BlockIndexMap,
    records: Vec<Option<Mips4BlockRecord<B::CompiledBlock, B::CompiledRegion>>>,
    free_records: Vec<usize>,
    dispatch_cache: Vec<Option<Mips4BlockDispatchEntry>>,
    last_dispatch: Cell<Option<Mips4BlockDispatchEntry>>,
    region_count: usize,
    statistics: Mips4BlockEngineStatistics,
}

impl<B> Mips4BlockEngine<B>
where
    B: Mips4CodegenBackend,
{
    /// Creates an empty engine around a native backend.
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            indices: HashMap::default(),
            records: Vec::new(),
            free_records: Vec::new(),
            dispatch_cache: vec![None; MIPS4_BLOCK_DISPATCH_CACHE_CAPACITY],
            last_dispatch: Cell::new(None),
            region_count: 0,
            statistics: Mips4BlockEngineStatistics::default(),
        }
    }

    /// Returns a cached block for guard validation.
    pub fn block(&self, key: Mips4BlockKey) -> Option<&Mips4Block> {
        let index = self.record_index(key)?;
        self.records
            .get(index)
            .and_then(Option::as_ref)
            .map(|record| &record.block)
    }

    /// Returns whether the engine contains a given key.
    pub fn contains(&self, key: Mips4BlockKey) -> bool {
        self.record_index(key).is_some()
    }

    /// Returns whether a cached block must observe synchronized CP0 counters.
    pub fn counter_barrier(&self, key: Mips4BlockKey) -> Option<bool> {
        let index = self.record_index(key)?;
        self.records
            .get(index)
            .and_then(Option::as_ref)
            .map(|record| record.counter_barrier)
    }

    /// Inserts a verified block, resetting the derived cache at capacity.
    pub fn insert(&mut self, block: Mips4Block) -> Result<(), Mips4BlockEngineError<B::Error>> {
        block
            .verify()
            .map_err(Mips4BlockEngineError::InvalidBlock)?;
        if self.indices.len() == MIPS4_BLOCK_CACHE_CAPACITY {
            self.reset()?;
        }
        let runtime_operations = block.runtime_operations();
        let runtime_descriptors = runtime_operations
            .iter()
            .copied()
            .map(Mips4RuntimeOperationDescriptor::from_operation)
            .collect();
        let counter_barrier = runtime_operations
            .iter()
            .any(|operation| matches!(operation, Mips4RuntimeOperation::Cp0 { .. }));
        let guard_mutating = runtime_operations
            .iter()
            .any(|operation| matches!(operation, Mips4RuntimeOperation::Cache { .. }));
        let key = block.key();
        let record = Mips4BlockRecord {
            block,
            runtime_operations,
            runtime_descriptors,
            counter_barrier,
            guard_mutating,
            guard_validation_epoch: None,
            operation_hotness: 0,
            compiled: None,
            region_hotness: 0,
            region_next_compile_hotness: MIPS4_REGION_HOT_THRESHOLD,
            successors: [Mips4SuccessorProfile::default(); 4],
            compiled_region: None,
        };
        if let Some(index) = self.indices.get(&key).copied() {
            self.invalidate_regions_depending_on(key);
            if self.records[index]
                .as_ref()
                .is_some_and(|record| record.compiled_region.is_some())
            {
                self.region_count = self.region_count.saturating_sub(1);
            }
            self.records[index] = Some(record);
            self.install_dispatch_entry(key, index);
        } else {
            let index = self.free_records.pop().unwrap_or_else(|| {
                self.records.push(None);
                self.records.len() - 1
            });
            self.records[index] = Some(record);
            self.indices.insert(key, index);
            self.install_dispatch_entry(key, index);
        }
        Ok(())
    }

    /// Inserts a transaction-fetched single-instruction block while preserving
    /// hotness and native code when the fetched instruction is unchanged.
    pub fn insert_dynamic(
        &mut self,
        block: Mips4Block,
    ) -> Result<(), Mips4BlockEngineError<B::Error>> {
        block
            .verify()
            .map_err(Mips4BlockEngineError::InvalidBlock)?;
        self.statistics.dynamic_fetches = self.statistics.dynamic_fetches.saturating_add(1);
        if self
            .block(block.key())
            .map(|cached| cached == &block)
            .unwrap_or(false)
        {
            return Ok(());
        }
        self.insert(block)
    }

    /// Records instructions supplied by a stable external code window.
    pub fn record_fast_fetches(&mut self, instructions: u64, source: Mips4CodeGuardKind) {
        self.statistics.fast_fetches = self.statistics.fast_fetches.saturating_add(instructions);
        match source {
            Mips4CodeGuardKind::SystemFlash => {
                self.statistics.system_flash_fetches = self
                    .statistics
                    .system_flash_fetches
                    .saturating_add(instructions);
            }
            Mips4CodeGuardKind::Sdram => {
                self.statistics.sdram_fetches =
                    self.statistics.sdram_fetches.saturating_add(instructions);
            }
        }
    }

    /// Removes one block after a failed visibility guard.
    pub fn invalidate(&mut self, key: Mips4BlockKey) -> bool {
        self.remove_dispatch_entry(key);
        let removed = if let Some(index) = self.indices.remove(&key) {
            let record = self.records[index].take();
            if record
                .as_ref()
                .is_some_and(|record| record.compiled_region.is_some())
            {
                self.region_count = self.region_count.saturating_sub(1);
            }
            let removed = record.is_some();
            if removed {
                self.free_records.push(index);
            }
            removed
        } else {
            false
        };
        self.invalidate_regions_depending_on(key);
        if removed {
            self.statistics.invalidated_blocks =
                self.statistics.invalidated_blocks.saturating_add(1);
        }
        removed
    }

    fn invalidate_regions_depending_on(&mut self, key: Mips4BlockKey) {
        for record in self.records.iter_mut().flatten() {
            let depends_on_key = record
                .compiled_region
                .as_ref()
                .is_some_and(|region| region.member_keys.contains(&key));
            if depends_on_key {
                record.compiled_region = None;
                record.region_next_compile_hotness = record.region_hotness;
                self.region_count = self.region_count.saturating_sub(1);
            }
        }
    }

    fn maybe_compile_region(
        &mut self,
        entry_index: usize,
    ) -> Result<(), Mips4BlockEngineError<B::Error>> {
        let Some(record) = self.records.get(entry_index).and_then(Option::as_ref) else {
            return Ok(());
        };
        if !region_compile_due(record) {
            return Ok(());
        }
        if self.region_count == MIPS4_REGION_CACHE_CAPACITY {
            self.reset()?;
            return Ok(());
        }
        let Some(region) = build_profiled_region(entry_index, &self.indices, &self.records) else {
            if let Some(record) = self.records.get_mut(entry_index).and_then(Option::as_mut) {
                record.region_next_compile_hotness = record
                    .region_hotness
                    .saturating_add(MIPS4_REGION_RETRY_OPERATIONS);
            }
            return Ok(());
        };
        let runtime_operations = region.runtime_operations();
        let runtime_descriptors = runtime_operations
            .iter()
            .copied()
            .map(Mips4RuntimeOperationDescriptor::from_operation)
            .collect();
        let member_keys = region.member_keys();
        let guards = region.guards();
        let entry_key = region.key().entry;
        let compiled = self
            .backend
            .compile_region(&region)
            .map_err(Mips4BlockEngineError::Backend)?;
        let Some(record) = self.records.get_mut(entry_index).and_then(Option::as_mut) else {
            return Err(Mips4BlockEngineError::MissingBlock(Box::new(entry_key)));
        };
        if record.block.key() != entry_key || record.compiled_region.is_some() {
            return Ok(());
        }
        record.compiled_region = Some(Mips4CompiledRegionRecord {
            compiled,
            runtime_operations,
            runtime_descriptors,
            member_keys,
            guards,
            guard_validation_epoch: None,
        });
        record.region_next_compile_hotness = u64::MAX;
        self.region_count += 1;
        self.statistics.region_compilations = self.statistics.region_compilations.saturating_add(1);
        Ok(())
    }

    /// Executes one cached block and performs deterministic tier promotion.
    pub fn execute(
        &mut self,
        key: Mips4BlockKey,
        frame: &mut Mips4BlockFrame,
    ) -> Result<Mips4BlockExecution, Mips4BlockEngineError<B::Error>> {
        struct RejectRuntime;

        impl Mips4BlockRuntime for RejectRuntime {
            fn execute(
                &mut self,
                _frame: &mut Mips4BlockFrame,
                _operation: Mips4RuntimeOperation,
            ) -> Mips4RuntimeResult {
                Mips4RuntimeResult::InternalError
            }
        }

        self.execute_with_runtime(key, frame, &mut RejectRuntime)
    }

    /// Executes one cached block with access to typed shared runtime semantics.
    pub fn execute_with_runtime<R>(
        &mut self,
        key: Mips4BlockKey,
        frame: &mut Mips4BlockFrame,
        runtime: &mut R,
    ) -> Result<Mips4BlockExecution, Mips4BlockEngineError<B::Error>>
    where
        R: Mips4BlockRuntime,
    {
        let index = self
            .record_index(key)
            .ok_or_else(|| Mips4BlockEngineError::MissingBlock(Box::new(key)))?;
        frame.operations_executed = 0;
        frame.runtime_calls = 0;
        frame.operation_base = 0;
        frame.runtime_call_base = 0;
        frame.region_side_exit = 0;
        let (exit, tier, counter_barrier, promoted) = {
            let (backend, records) = (&mut self.backend, &mut self.records);
            let record = records[index]
                .as_mut()
                .ok_or_else(|| Mips4BlockEngineError::MissingBlock(Box::new(key)))?;
            if let Some(region) = &record.compiled_region {
                let result = if region.runtime_operations.is_empty() {
                    backend.execute_region(&region.compiled, frame)
                } else {
                    frame.install_runtime(
                        runtime,
                        &region.runtime_operations,
                        &region.runtime_descriptors,
                    );
                    let result = backend.execute_region(&region.compiled, frame);
                    frame.clear_runtime();
                    result
                };
                (
                    result.map_err(Mips4BlockEngineError::Backend)?,
                    Mips4BlockTier::Region,
                    record.counter_barrier,
                    false,
                )
            } else if let Some(compiled) = &record.compiled {
                let result = if record.runtime_operations.is_empty() {
                    backend.execute(compiled, frame)
                } else {
                    frame.install_runtime(
                        runtime,
                        &record.runtime_operations,
                        &record.runtime_descriptors,
                    );
                    let result = backend.execute(compiled, frame);
                    frame.clear_runtime();
                    result
                };
                (
                    result.map_err(Mips4BlockEngineError::Backend)?,
                    Mips4BlockTier::Native,
                    record.counter_barrier,
                    false,
                )
            } else {
                let (exit, promoted) = execute_interpreted_tier(backend, record, frame, runtime)?;
                (
                    exit,
                    Mips4BlockTier::Interpreter,
                    record.counter_barrier,
                    promoted,
                )
            }
        };

        if tier == Mips4BlockTier::Region && exit == Mips4BlockExit::BudgetExhausted {
            frame.region_side_exit = Mips4RegionSideExit::Budget as u64;
        }
        let region_side_exit = if tier == Mips4BlockTier::Region {
            frame.region_side_exit()
        } else {
            None
        };

        match tier {
            Mips4BlockTier::Interpreter => {
                self.statistics.interpreted_blocks =
                    self.statistics.interpreted_blocks.saturating_add(1);
                self.statistics.interpreted_operations = self
                    .statistics
                    .interpreted_operations
                    .saturating_add(frame.operations_executed);
                if promoted {
                    self.statistics.compiled_blocks =
                        self.statistics.compiled_blocks.saturating_add(1);
                }
            }
            Mips4BlockTier::Native => {
                self.statistics.native_blocks = self.statistics.native_blocks.saturating_add(1);
                self.statistics.native_operations = self
                    .statistics
                    .native_operations
                    .saturating_add(frame.operations_executed);
            }
            Mips4BlockTier::Region => {
                self.statistics.region_entries = self.statistics.region_entries.saturating_add(1);
                self.statistics.region_operations = self
                    .statistics
                    .region_operations
                    .saturating_add(frame.operations_executed);
                record_region_side_exit(&mut self.statistics, region_side_exit);
            }
        }
        self.statistics.runtime_calls = self
            .statistics
            .runtime_calls
            .saturating_add(frame.runtime_calls);

        if tier != Mips4BlockTier::Region {
            let record = self.records[index]
                .as_mut()
                .ok_or_else(|| Mips4BlockEngineError::MissingBlock(Box::new(key)))?;
            record.region_hotness = record
                .region_hotness
                .saturating_add(frame.operations_executed);
            if exit == Mips4BlockExit::Dispatch {
                let mut successor = key;
                successor.pc = frame.pc();
                successor.next_pc = frame.next_pc();
                successor.delay_slot_branch_pc = frame.delay_slot_branch_pc();
                record_successor(record, successor);
            }
            if region_compile_due(record) {
                self.maybe_compile_region(index)?;
            }
        }

        Ok(Mips4BlockExecution {
            exit,
            tier,
            counter_barrier,
            operations_executed: frame.operations_executed,
            runtime_calls: frame.runtime_calls,
            region_side_exit,
        })
    }

    /// Executes one reusable I-cache block with a single record lookup.
    pub fn execute_cached_with_runtime<R>(
        &mut self,
        key: Mips4BlockKey,
        frame: &mut Mips4BlockFrame,
        runtime: &mut R,
        counters_dirty: bool,
    ) -> Result<Mips4CachedBlockExecution, Mips4BlockEngineError<B::Error>>
    where
        R: Mips4BlockRuntime,
    {
        if self.region_count == MIPS4_REGION_CACHE_CAPACITY {
            self.reset()?;
            return Ok(Mips4CachedBlockExecution::Missing);
        }
        let mut should_compile_region = false;
        let mut region_guard_side_exit = false;
        let (execution, invalidate) = {
            let Some(index) = self.record_index(key) else {
                return Ok(Mips4CachedBlockExecution::Missing);
            };
            let (backend, records) = (&mut self.backend, &mut self.records);
            let Some(record) = records[index].as_mut() else {
                return Ok(Mips4CachedBlockExecution::Missing);
            };
            let reusable_source = !record.block.guard().lines().is_empty()
                && record.block.guard().code_source().is_none();
            let block_guard_valid = reusable_source && cached_guard_valid(record, runtime);
            let region_guard_valid = record
                .compiled_region
                .as_mut()
                .is_none_or(|region| compiled_region_guard_valid(region, runtime));
            if !block_guard_valid || !region_guard_valid {
                if record.compiled_region.is_some() {
                    region_guard_side_exit = true;
                    frame.region_side_exit = Mips4RegionSideExit::Guard as u64;
                }
                (Mips4CachedBlockExecution::Missing, reusable_source)
            } else if counters_dirty && record.counter_barrier {
                (Mips4CachedBlockExecution::CounterSynchronization, false)
            } else {
                frame.operations_executed = 0;
                frame.runtime_calls = 0;
                frame.operation_base = 0;
                frame.runtime_call_base = 0;
                frame.region_side_exit = 0;
                let (exit, tier) = if let Some(region) = &record.compiled_region {
                    let result = if region.runtime_operations.is_empty() {
                        backend.execute_region(&region.compiled, frame)
                    } else {
                        frame.install_runtime(
                            runtime,
                            &region.runtime_operations,
                            &region.runtime_descriptors,
                        );
                        let result = backend.execute_region(&region.compiled, frame);
                        frame.clear_runtime();
                        result
                    };
                    let exit = result.map_err(Mips4BlockEngineError::Backend)?;
                    (exit, Mips4BlockTier::Region)
                } else if let Some(compiled) = &record.compiled {
                    let result = if record.runtime_operations.is_empty() {
                        backend.execute(compiled, frame)
                    } else {
                        frame.install_runtime(
                            runtime,
                            &record.runtime_operations,
                            &record.runtime_descriptors,
                        );
                        let result = backend.execute(compiled, frame);
                        frame.clear_runtime();
                        result
                    };
                    let exit = result.map_err(Mips4BlockEngineError::Backend)?;
                    (exit, Mips4BlockTier::Native)
                } else {
                    let (exit, promoted) =
                        execute_interpreted_tier(backend, record, frame, runtime)?;
                    if promoted {
                        self.statistics.compiled_blocks =
                            self.statistics.compiled_blocks.saturating_add(1);
                    }
                    (exit, Mips4BlockTier::Interpreter)
                };

                record.region_hotness = record
                    .region_hotness
                    .saturating_add(frame.operations_executed);
                if tier != Mips4BlockTier::Region && exit == Mips4BlockExit::Dispatch {
                    let mut successor = key;
                    successor.pc = frame.pc();
                    successor.next_pc = frame.next_pc();
                    successor.delay_slot_branch_pc = frame.delay_slot_branch_pc();
                    record_successor(record, successor);
                }
                should_compile_region =
                    tier != Mips4BlockTier::Region && region_compile_due(record);

                if tier == Mips4BlockTier::Region && exit == Mips4BlockExit::BudgetExhausted {
                    frame.region_side_exit = Mips4RegionSideExit::Budget as u64;
                }
                let region_side_exit = if tier == Mips4BlockTier::Region {
                    frame.region_side_exit()
                } else {
                    None
                };

                (
                    Mips4CachedBlockExecution::Executed(Mips4BlockExecution {
                        exit,
                        tier,
                        counter_barrier: record.counter_barrier,
                        operations_executed: frame.operations_executed,
                        runtime_calls: frame.runtime_calls,
                        region_side_exit,
                    }),
                    record.guard_mutating && !cached_guard_valid(record, runtime),
                )
            }
        };
        if region_guard_side_exit {
            self.statistics.region_guard_side_exits =
                self.statistics.region_guard_side_exits.saturating_add(1);
        }
        if invalidate {
            debug_assert!(self.invalidate(key));
        } else if should_compile_region {
            let index = self
                .record_index(key)
                .ok_or_else(|| Mips4BlockEngineError::MissingBlock(Box::new(key)))?;
            self.maybe_compile_region(index)?;
        }
        Ok(execution)
    }

    /// Clears IR, hotness, and native code at a dispatcher-safe point.
    pub fn reset(&mut self) -> Result<(), Mips4BlockEngineError<B::Error>> {
        self.indices.clear();
        self.records.clear();
        self.free_records.clear();
        self.dispatch_cache.fill(None);
        self.last_dispatch.set(None);
        self.region_count = 0;
        self.backend
            .clear()
            .map_err(Mips4BlockEngineError::Backend)?;
        self.statistics.cache_resets = self.statistics.cache_resets.saturating_add(1);
        Ok(())
    }

    /// Returns current derived performance counters.
    pub const fn statistics(&self) -> Mips4BlockEngineStatistics {
        self.statistics
    }

    /// Returns whether a valid reusable I-cache block exists for one key.
    pub fn reusable_instruction_cache_block<R>(&mut self, key: Mips4BlockKey, runtime: &R) -> bool
    where
        R: Mips4BlockRuntime,
    {
        let Some(index) = self.record_index(key) else {
            return false;
        };
        self.records[index].as_mut().is_some_and(|record| {
            !record.block.guard().lines().is_empty()
                && record.block.guard().code_source().is_none()
                && key.code_guard == 0
                && cached_guard_valid(record, runtime)
        })
    }

    /// Commits execution counters accumulated by a cached-block dispatcher.
    pub fn record_cached_statistics(&mut self, statistics: Mips4BlockEngineStatistics) {
        self.statistics.interpreted_blocks = self
            .statistics
            .interpreted_blocks
            .saturating_add(statistics.interpreted_blocks);
        self.statistics.native_blocks = self
            .statistics
            .native_blocks
            .saturating_add(statistics.native_blocks);
        self.statistics.interpreted_operations = self
            .statistics
            .interpreted_operations
            .saturating_add(statistics.interpreted_operations);
        self.statistics.native_operations = self
            .statistics
            .native_operations
            .saturating_add(statistics.native_operations);
        self.statistics.runtime_calls = self
            .statistics
            .runtime_calls
            .saturating_add(statistics.runtime_calls);
        self.statistics.region_entries = self
            .statistics
            .region_entries
            .saturating_add(statistics.region_entries);
        self.statistics.region_operations = self
            .statistics
            .region_operations
            .saturating_add(statistics.region_operations);
        self.statistics.region_cold_side_exits = self
            .statistics
            .region_cold_side_exits
            .saturating_add(statistics.region_cold_side_exits);
        self.statistics.region_budget_side_exits = self
            .statistics
            .region_budget_side_exits
            .saturating_add(statistics.region_budget_side_exits);
        self.statistics.region_runtime_side_exits = self
            .statistics
            .region_runtime_side_exits
            .saturating_add(statistics.region_runtime_side_exits);
        self.statistics.region_guard_side_exits = self
            .statistics
            .region_guard_side_exits
            .saturating_add(statistics.region_guard_side_exits);
    }

    /// Returns the number of cached block records.
    pub fn len(&self) -> usize {
        self.indices.len()
    }

    /// Returns whether no block record is cached.
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    #[inline(always)]
    fn record_index(&self, key: Mips4BlockKey) -> Option<usize> {
        if let Some(entry) = self.last_dispatch.get()
            && entry.key == key
        {
            return Some(entry.record);
        }
        let slot = dispatch_slot(key);
        if let Some(entry) = self.dispatch_cache[slot]
            && entry.key == key
        {
            self.last_dispatch.set(Some(entry));
            return Some(entry.record);
        }
        let record = self.indices.get(&key).copied();
        self.last_dispatch
            .set(record.map(|record| Mips4BlockDispatchEntry { key, record }));
        record
    }

    fn install_dispatch_entry(&mut self, key: Mips4BlockKey, record: usize) {
        let entry = Mips4BlockDispatchEntry { key, record };
        self.dispatch_cache[dispatch_slot(key)] = Some(entry);
        self.last_dispatch.set(Some(entry));
    }

    fn remove_dispatch_entry(&mut self, key: Mips4BlockKey) {
        if self
            .last_dispatch
            .get()
            .is_some_and(|entry| entry.key == key)
        {
            self.last_dispatch.set(None);
        }
        let entry = &mut self.dispatch_cache[dispatch_slot(key)];
        if entry.is_some_and(|entry| entry.key == key) {
            *entry = None;
        }
    }
}

const fn dispatch_slot(key: Mips4BlockKey) -> usize {
    ((key.pc >> 2) as usize) & (MIPS4_BLOCK_DISPATCH_CACHE_CAPACITY - 1)
}

/// Tiered block-engine failure.
#[derive(Debug)]
pub enum Mips4BlockEngineError<E> {
    /// A block failed structural verification before insertion.
    InvalidBlock(Mips4BlockBuildError),

    /// A requested block key was not present.
    MissingBlock(Box<Mips4BlockKey>),

    /// The native backend failed.
    Backend(E),
}

impl<E> fmt::Display for Mips4BlockEngineError<E>
where
    E: fmt::Display,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBlock(error) => error.fmt(formatter),
            Self::MissingBlock(key) => write!(formatter, "MIPS IV block {key:?} is not cached"),
            Self::Backend(error) => write!(formatter, "MIPS IV code generation failed: {error}"),
        }
    }
}

impl<E> std::error::Error for Mips4BlockEngineError<E> where E: std::error::Error + 'static {}

fn interpret_operation<R>(
    operation: Mips4BlockOperation,
    frame: &mut Mips4BlockFrame,
    runtime: &mut R,
) -> Result<Mips4RuntimeResult, Mips4BlockException>
where
    R: Mips4BlockRuntime + ?Sized,
{
    match operation {
        Mips4BlockOperation::Arithmetic {
            operation,
            width,
            trap_on_overflow,
            noop_on_invalid_word,
            destination,
            lhs,
            rhs,
        } => {
            let lhs = frame.read_gpr(lhs);
            let rhs_operand = rhs;
            let rhs = operand(frame, rhs_operand);
            if width == Mips4BlockWidth::Word
                && noop_on_invalid_word
                && (!is_sign_extended_word(lhs)
                    || matches!(
                        rhs_operand,
                        Mips4BlockOperand::Register(_) if !is_sign_extended_word(rhs)
                    ))
            {
                return Ok(Mips4RuntimeResult::Continue);
            }
            let value = match (operation, width, trap_on_overflow) {
                (Mips4BlockArithmetic::Add, Mips4BlockWidth::Word, true) => {
                    Mips4Alu::add(lhs, rhs).map_err(block_exception)?
                }
                (Mips4BlockArithmetic::Add, Mips4BlockWidth::Word, false) => {
                    Mips4Alu::addu(lhs, rhs)
                }
                (Mips4BlockArithmetic::Subtract, Mips4BlockWidth::Word, true) => {
                    Mips4Alu::sub(lhs, rhs).map_err(block_exception)?
                }
                (Mips4BlockArithmetic::Subtract, Mips4BlockWidth::Word, false) => {
                    Mips4Alu::subu(lhs, rhs)
                }
                (Mips4BlockArithmetic::Add, Mips4BlockWidth::Doubleword, true) => {
                    Mips4Alu::dadd(lhs, rhs).map_err(block_exception)?
                }
                (Mips4BlockArithmetic::Add, Mips4BlockWidth::Doubleword, false) => {
                    Mips4Alu::daddu(lhs, rhs)
                }
                (Mips4BlockArithmetic::Subtract, Mips4BlockWidth::Doubleword, true) => {
                    Mips4Alu::dsub(lhs, rhs).map_err(block_exception)?
                }
                (Mips4BlockArithmetic::Subtract, Mips4BlockWidth::Doubleword, false) => {
                    Mips4Alu::dsubu(lhs, rhs)
                }
            };
            frame.write_gpr(destination, value);
        }
        Mips4BlockOperation::Logical {
            operation,
            destination,
            lhs,
            rhs,
        } => {
            let lhs = frame.read_gpr(lhs);
            let rhs = operand(frame, rhs);
            let value = match operation {
                Mips4BlockLogical::And => Mips4Alu::and(lhs, rhs),
                Mips4BlockLogical::Or => Mips4Alu::or(lhs, rhs),
                Mips4BlockLogical::Xor => Mips4Alu::xor(lhs, rhs),
                Mips4BlockLogical::Nor => Mips4Alu::nor(lhs, rhs),
            };
            frame.write_gpr(destination, value);
        }
        Mips4BlockOperation::LoadUpperImmediate {
            destination,
            immediate,
        } => frame.write_gpr(destination, Mips4Alu::lui(immediate)),
        Mips4BlockOperation::Shift {
            operation,
            width,
            noop_on_invalid_word,
            destination,
            value,
            amount,
        } => {
            let value = frame.read_gpr(value);
            if width == Mips4BlockWidth::Word
                && noop_on_invalid_word
                && !is_sign_extended_word(value)
            {
                return Ok(Mips4RuntimeResult::Continue);
            }
            let amount = match amount {
                Mips4BlockShiftAmount::Immediate(amount) => u64::from(amount),
                Mips4BlockShiftAmount::Register(register) => frame.read_gpr(register),
            };
            let result = match (operation, width) {
                (Mips4BlockShift::Left, Mips4BlockWidth::Word) => Mips4Alu::sllv(value, amount),
                (Mips4BlockShift::RightLogical, Mips4BlockWidth::Word) => {
                    Mips4Alu::srlv(value, amount)
                }
                (Mips4BlockShift::RightArithmetic, Mips4BlockWidth::Word) => {
                    Mips4Alu::srav(value, amount)
                }
                (Mips4BlockShift::Left, Mips4BlockWidth::Doubleword) => {
                    Mips4Alu::dsllv(value, amount)
                }
                (Mips4BlockShift::RightLogical, Mips4BlockWidth::Doubleword) => {
                    Mips4Alu::dsrlv(value, amount)
                }
                (Mips4BlockShift::RightArithmetic, Mips4BlockWidth::Doubleword) => {
                    Mips4Alu::dsrav(value, amount)
                }
            };
            frame.write_gpr(destination, result);
        }
        Mips4BlockOperation::Compare {
            comparison,
            destination,
            lhs,
            rhs,
        } => {
            let lhs = frame.read_gpr(lhs);
            let rhs = operand(frame, rhs);
            let result = match comparison {
                Mips4BlockComparison::SignedLessThan => Mips4Alu::slt(lhs, rhs),
                Mips4BlockComparison::UnsignedLessThan => Mips4Alu::sltu(lhs, rhs),
            };
            frame.write_gpr(destination, result);
        }
        Mips4BlockOperation::Multiply {
            width,
            signed,
            noop_on_invalid_word,
            lhs,
            rhs,
        } => {
            let lhs = frame.read_gpr(lhs);
            let rhs = frame.read_gpr(rhs);
            if width == Mips4BlockWidth::Word
                && noop_on_invalid_word
                && (!is_sign_extended_word(lhs) || !is_sign_extended_word(rhs))
            {
                return Ok(Mips4RuntimeResult::Continue);
            }
            let result = match (width, signed) {
                (Mips4BlockWidth::Word, true) => Mips4Alu::mult(lhs, rhs),
                (Mips4BlockWidth::Word, false) => Mips4Alu::multu(lhs, rhs),
                (Mips4BlockWidth::Doubleword, true) => Mips4Alu::dmult(lhs, rhs),
                (Mips4BlockWidth::Doubleword, false) => Mips4Alu::dmultu(lhs, rhs),
            };
            frame.hi = result.hi;
            frame.lo = result.lo;
        }
        Mips4BlockOperation::Divide {
            width,
            signed,
            noop_on_invalid_word,
            lhs,
            rhs,
        } => {
            let lhs = frame.read_gpr(lhs);
            let rhs = frame.read_gpr(rhs);
            if width == Mips4BlockWidth::Word
                && noop_on_invalid_word
                && (!is_sign_extended_word(lhs) || !is_sign_extended_word(rhs))
            {
                return Ok(Mips4RuntimeResult::Continue);
            }
            let result = match (width, signed) {
                (Mips4BlockWidth::Word, true) => Mips4Alu::div(lhs, rhs),
                (Mips4BlockWidth::Word, false) => Mips4Alu::divu(lhs, rhs),
                (Mips4BlockWidth::Doubleword, true) => Mips4Alu::ddiv(lhs, rhs),
                (Mips4BlockWidth::Doubleword, false) => Mips4Alu::ddivu(lhs, rhs),
            };
            if let Some(result) = result {
                frame.hi = result.hi;
                frame.lo = result.lo;
            }
        }
        Mips4BlockOperation::MoveFromSpecial { high, destination } => {
            frame.write_gpr(destination, if high { frame.hi } else { frame.lo });
        }
        Mips4BlockOperation::MoveToSpecial { high, source } => {
            let value = frame.read_gpr(source);
            if high {
                frame.hi = value;
            } else {
                frame.lo = value;
            }
        }
        Mips4BlockOperation::ConditionalMove {
            when_zero,
            destination,
            source,
            condition,
        } => {
            let condition = frame.read_gpr(condition);
            if (condition == 0) == when_zero {
                frame.write_gpr(destination, frame.read_gpr(source));
            }
        }
        Mips4BlockOperation::Trap { trap, lhs, rhs } => {
            let lhs = frame.read_gpr(lhs);
            let rhs = operand(frame, rhs);
            let decision = match trap {
                Mips4BlockTrap::Equal => teq(lhs, rhs),
                Mips4BlockTrap::NotEqual => tne(lhs, rhs),
                Mips4BlockTrap::SignedGreaterThanOrEqual => tge(lhs, rhs),
                Mips4BlockTrap::UnsignedGreaterThanOrEqual => tgeu(lhs, rhs),
                Mips4BlockTrap::SignedLessThan => tlt(lhs, rhs),
                Mips4BlockTrap::UnsignedLessThan => tltu(lhs, rhs),
            };
            if matches!(decision, Mips4TrapDecision::Trap) {
                return Err(Mips4BlockException::Trap);
            }
        }
        Mips4BlockOperation::Exception(exception) => return Err(exception),
        Mips4BlockOperation::Runtime(operation) => {
            frame.runtime_calls = frame.runtime_calls.saturating_add(1);
            return Ok(runtime.execute(frame, operation));
        }
        Mips4BlockOperation::NoOperation => {}
    }
    Ok(Mips4RuntimeResult::Continue)
}

fn operand(frame: &Mips4BlockFrame, operand: Mips4BlockOperand) -> u64 {
    match operand {
        Mips4BlockOperand::Register(register) => frame.read_gpr(register),
        Mips4BlockOperand::SignedImmediate(immediate) => immediate as i64 as u64,
        Mips4BlockOperand::UnsignedImmediate(immediate) => u64::from(immediate),
    }
}

fn block_exception(exception: Mips4Exception) -> Mips4BlockException {
    match exception {
        Mips4Exception::ArithmeticOverflow => Mips4BlockException::ArithmeticOverflow,
        _ => unreachable!(),
    }
}

fn branch_condition(condition: Mips4BlockBranchCondition, frame: &Mips4BlockFrame) -> bool {
    match condition {
        Mips4BlockBranchCondition::Always => true,
        Mips4BlockBranchCondition::Equal { lhs, rhs } => frame.read_gpr(lhs) == frame.read_gpr(rhs),
        Mips4BlockBranchCondition::NotEqual { lhs, rhs } => {
            frame.read_gpr(lhs) != frame.read_gpr(rhs)
        }
        Mips4BlockBranchCondition::LessThanZero { source } => (frame.read_gpr(source) as i64) < 0,
        Mips4BlockBranchCondition::GreaterThanOrEqualZero { source } => {
            (frame.read_gpr(source) as i64) >= 0
        }
        Mips4BlockBranchCondition::LessThanOrEqualZero { source } => {
            (frame.read_gpr(source) as i64) <= 0
        }
        Mips4BlockBranchCondition::GreaterThanZero { source } => {
            (frame.read_gpr(source) as i64) > 0
        }
    }
}

fn retire(frame: &mut Mips4BlockFrame) -> bool {
    frame.retired += 1;
    frame.budget -= 1;
    frame.budget == 0
}

fn retire_sequential(frame: &mut Mips4BlockFrame) {
    frame.pc = frame.next_pc;
    frame.next_pc = frame.next_pc.wrapping_add(4);
    frame.delay_slot_branch_pc = 0;
    frame.delay_slot_valid = 0;
}

fn retire_branch(
    frame: &mut Mips4BlockFrame,
    branch_pc: u64,
    target: u64,
    taken: bool,
    likely: bool,
) {
    if !taken && likely {
        frame.pc = frame.next_pc.wrapping_add(4);
        frame.next_pc = frame.pc.wrapping_add(4);
        frame.delay_slot_branch_pc = 0;
        frame.delay_slot_valid = 0;
        return;
    }
    frame.pc = frame.next_pc;
    frame.next_pc = if taken {
        target
    } else {
        frame.next_pc.wrapping_add(4)
    };
    frame.delay_slot_branch_pc = branch_pc;
    frame.delay_slot_valid = 1;
}

#[cfg(test)]
mod tests {
    use crate::cpu::mips4::config::{Mips4CacheConfig, Mips4Endianness};
    use crate::cpu::mips4::instruction::decode::{
        Mips4InstructionClass, Mips4InstructionDecode, decode_instruction,
    };
    use crate::cpu::mips4::model::r5000::boot_mode::R5000BootMode;
    use crate::cpu::mips4::model::r5000::execution_policy::R5000ExecutionPolicy;
    use crate::cpu::mips4::model::r5000::profile::R5000Profile;
    use crate::cpu::mips4::model::r5000::revision::R5000Revision;

    use super::*;

    #[test]
    fn reciprocal_division_matches_unsigned_division() {
        fn divide(numerator: u64, divisor: u64) -> u64 {
            let reciprocal = division_reciprocal(divisor);
            let quotient = ((u128::from(numerator) * u128::from(reciprocal)) >> 64) as u64;
            let remainder = numerator - quotient * divisor;
            quotient + u64::from(remainder >= divisor)
        }

        let divisors = [
            2,
            3,
            7,
            9,
            10,
            66_000_000,
            100_000_000,
            180_000_000,
            1_000_000_000,
            u32::MAX as u64,
            u64::MAX,
        ];
        let mut random = 0x6a09_e667_f3bc_c909_u64;
        for divisor in divisors {
            for numerator in [
                0,
                1,
                divisor - 1,
                divisor,
                divisor.saturating_add(1),
                u64::MAX,
            ] {
                assert_eq!(divide(numerator, divisor), numerator / divisor);
            }
            for _ in 0..10_000 {
                random ^= random << 13;
                random ^= random >> 7;
                random ^= random << 17;
                assert_eq!(divide(random, divisor), random / divisor);
            }
        }
    }

    fn policy() -> R5000ExecutionPolicy {
        R5000ExecutionPolicy::new(
            R5000Profile::new(
                Mips4Endianness::Big,
                R5000Revision::from_bits(0x21),
                180_000_000,
                Mips4CacheConfig::present(32 * 1024, 32),
                Mips4CacheConfig::present(32 * 1024, 32),
                Mips4CacheConfig::disabled(),
            ),
            R5000BootMode::from_low_bits(0).unwrap(),
        )
    }

    fn metadata(pc: u64, instruction: u32) -> Mips4BlockInstructionMetadata {
        Mips4BlockInstructionMetadata {
            pc,
            instruction,
            delay_slot_branch_pc: None,
        }
    }

    fn lift(pc: u64, bits: u32) -> Mips4BlockLiftedInstruction {
        let raw = Mips4Instruction::from_bits(bits);
        let Mips4InstructionDecode::Instruction(Mips4InstructionClass::Cpu(decoded)) =
            decode_instruction(raw)
        else {
            panic!("test instruction must decode as a CPU instruction")
        };
        lift_cpu_instruction(&policy(), metadata(pc, bits), decoded)
    }

    #[test]
    fn interpreter_executes_integer_sequence() {
        let key = Mips4BlockKey {
            pc: 0x1000,
            next_pc: 0x1004,
            delay_slot_branch_pc: None,
            fetch_context: 0,
            translation_generation: 0,
            code_guard: 0,
        };
        let mut block = Mips4Block::new(key, Mips4BlockGuard::new());
        let addiu = (0x09_u32 << 26) | (1 << 21) | (2 << 16) | 7;
        let ori = (0x0d_u32 << 26) | (2 << 21) | (3 << 16) | 0x10;
        let Mips4BlockLiftedInstruction::Sequential(addiu) = lift(0x1000, addiu) else {
            panic!()
        };
        let Mips4BlockLiftedInstruction::Sequential(ori) = lift(0x1004, ori) else {
            panic!()
        };
        block.push(addiu).unwrap();
        block.push(ori).unwrap();
        block.terminate_dispatch().unwrap();
        let mut gpr = [0; MIPS4_GPR_COUNT];
        gpr[1] = 5;
        let mut frame = Mips4BlockFrame::new(gpr, 0, 0, 0x1000, 0x1004, None, 2);

        assert_eq!(
            interpret_block(&block, &mut frame),
            Mips4BlockExit::BudgetExhausted
        );
        assert_eq!(frame.read_gpr(2), 12);
        assert_eq!(frame.read_gpr(3), 28);
        assert_eq!(frame.pc(), 0x1008);
        assert_eq!(frame.retired(), 2);
    }

    #[test]
    fn verifier_requires_terminator_and_matching_retirement_metadata() {
        let key = Mips4BlockKey {
            pc: 0x1000,
            next_pc: 0x1004,
            delay_slot_branch_pc: None,
            fetch_context: 0,
            translation_generation: 0,
            code_guard: 0,
        };
        let mut block = Mips4Block::new(key, Mips4BlockGuard::new());
        let bits = (0x09_u32 << 26) | (1 << 21) | (2 << 16) | 7;
        let Mips4BlockLiftedInstruction::Sequential(instruction) = lift(0x1000, bits) else {
            panic!()
        };
        block.push(instruction).unwrap();
        assert_eq!(block.verify(), Err(Mips4BlockBuildError::MissingTerminator));
        block.terminate_dispatch().unwrap();
        block.body[0].retire.pc = 0x1004;
        assert_eq!(block.verify(), Err(Mips4BlockBuildError::InvalidRetirement));
    }

    #[test]
    fn branch_budget_preserves_delay_slot_restart_state() {
        let key = Mips4BlockKey {
            pc: 0x2000,
            next_pc: 0x2004,
            delay_slot_branch_pc: None,
            fetch_context: 0,
            translation_generation: 0,
            code_guard: 0,
        };
        let beq = (0x04_u32 << 26) | (1 << 21) | (1 << 16) | 3;
        let addiu = (0x09_u32 << 26) | (2 << 21) | (2 << 16) | 1;
        let Mips4BlockLiftedInstruction::Branch(branch) = lift(0x2000, beq) else {
            panic!()
        };
        let Mips4BlockLiftedInstruction::Sequential(mut delay) = lift(0x2004, addiu) else {
            panic!()
        };
        delay.metadata.delay_slot_branch_pc = Some(0x2000);
        let mut block = Mips4Block::new(key, Mips4BlockGuard::new());
        block.terminate_with_branch(branch, delay).unwrap();
        let mut frame = Mips4BlockFrame::new([0; MIPS4_GPR_COUNT], 0, 0, 0x2000, 0x2004, None, 1);

        assert_eq!(
            interpret_block(&block, &mut frame),
            Mips4BlockExit::BudgetExhausted
        );
        assert_eq!(frame.pc(), 0x2004);
        assert_eq!(frame.next_pc(), 0x2010);
        assert_eq!(frame.delay_slot_branch_pc(), Some(0x2000));
        assert_eq!(frame.read_gpr(2), 0);
    }
}
