//! Typed basic-block execution shared by the interpreter and native backends.

use core::{cell::Cell, fmt};
use se_core::scheduler::FractionalClockProjection;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::cpu::mips4::alu::Mips4Alu;
use crate::cpu::mips4::cp0::Mips4Cp0Register;
use crate::cpu::mips4::cp1::decode::{Mips4Cp1Decode, Mips4Cp1InstructionClass};
use crate::cpu::mips4::exception::{
    Mips4Exception, Mips4TrapDecision, teq, tge, tgeu, tlt, tltu, tne,
};
use crate::cpu::mips4::gpr::{MIPS4_GPR_COUNT, is_sign_extended_word};
use crate::cpu::mips4::instruction::Mips4Instruction;
use crate::cpu::mips4::instruction::decode::Mips4CpuInstruction;
use crate::cpu::mips4::instruction::requirements::Mips4InstructionRequirements;

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
            _ => None,
        }
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
    runtime_context: *mut (),
    runtime_call: usize,
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
/// Byte offset of the opaque runtime context in [`Mips4BlockFrame`].
pub const MIPS4_BLOCK_FRAME_RUNTIME_CONTEXT_OFFSET: i32 =
    core::mem::offset_of!(Mips4BlockFrame, runtime_context) as i32;
/// Byte offset of the runtime trampoline address in [`Mips4BlockFrame`].
pub const MIPS4_BLOCK_FRAME_RUNTIME_CALL_OFFSET: i32 =
    core::mem::offset_of!(Mips4BlockFrame, runtime_call) as i32;
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
            runtime_context: core::ptr::null_mut(),
            runtime_call: 0,
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
        self.runtime_operation_values = operation_values.as_ptr();
        self.runtime_operations = operations.as_ptr();
        self.runtime_operation_count = operations.len() as u64;
    }

    fn clear_runtime(&mut self) {
        self.runtime_context = core::ptr::null_mut();
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
pub trait Mips4CodegenBackend {
    /// Backend-owned compiled block handle.
    type CompiledBlock;

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
}

struct Mips4BlockRecord<C> {
    block: Mips4Block,
    runtime_operations: Vec<Mips4RuntimeOperation>,
    runtime_descriptors: Vec<Mips4RuntimeOperationDescriptor>,
    counter_barrier: bool,
    guard_mutating: bool,
    guard_validation_epoch: Option<u64>,
    operation_hotness: u64,
    compiled: Option<C>,
}

fn cached_guard_valid<C, R>(record: &mut Mips4BlockRecord<C>, runtime: &R) -> bool
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
    record: &mut Mips4BlockRecord<B::CompiledBlock>,
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
    records: Vec<Option<Mips4BlockRecord<B::CompiledBlock>>>,
    free_records: Vec<usize>,
    dispatch_cache: Vec<Option<Mips4BlockDispatchEntry>>,
    last_dispatch: Cell<Option<Mips4BlockDispatchEntry>>,
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
        };
        if let Some(index) = self.indices.get(&key).copied() {
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
    pub fn record_fast_fetches(&mut self, instructions: u64) {
        self.statistics.fast_fetches = self.statistics.fast_fetches.saturating_add(instructions);
    }

    /// Removes one block after a failed visibility guard.
    pub fn invalidate(&mut self, key: Mips4BlockKey) -> bool {
        self.remove_dispatch_entry(key);
        let removed = self.indices.remove(&key).is_some_and(|index| {
            let removed = self.records[index].take().is_some();
            if removed {
                self.free_records.push(index);
            }
            removed
        });
        if removed {
            self.statistics.invalidated_blocks =
                self.statistics.invalidated_blocks.saturating_add(1);
        }
        removed
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
        let (backend, records) = (&mut self.backend, &mut self.records);
        let record = records[index]
            .as_mut()
            .ok_or_else(|| Mips4BlockEngineError::MissingBlock(Box::new(key)))?;
        frame.operations_executed = 0;
        frame.runtime_calls = 0;
        let (exit, tier) = if let Some(compiled) = &record.compiled {
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
            self.statistics.native_blocks = self.statistics.native_blocks.saturating_add(1);
            (exit, Mips4BlockTier::Native)
        } else {
            let (exit, promoted) = execute_interpreted_tier(backend, record, frame, runtime)?;
            self.statistics.interpreted_blocks =
                self.statistics.interpreted_blocks.saturating_add(1);
            if promoted {
                self.statistics.compiled_blocks = self.statistics.compiled_blocks.saturating_add(1);
            }
            (exit, Mips4BlockTier::Interpreter)
        };

        match tier {
            Mips4BlockTier::Interpreter => {
                self.statistics.interpreted_operations = self
                    .statistics
                    .interpreted_operations
                    .saturating_add(frame.operations_executed);
            }
            Mips4BlockTier::Native => {
                self.statistics.native_operations = self
                    .statistics
                    .native_operations
                    .saturating_add(frame.operations_executed);
            }
        }
        self.statistics.runtime_calls = self
            .statistics
            .runtime_calls
            .saturating_add(frame.runtime_calls);

        Ok(Mips4BlockExecution {
            exit,
            tier,
            counter_barrier: record.counter_barrier,
            operations_executed: frame.operations_executed,
            runtime_calls: frame.runtime_calls,
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
        let (execution, invalidate) = {
            let Some(index) = self.record_index(key) else {
                return Ok(Mips4CachedBlockExecution::Missing);
            };
            let (backend, records) = (&mut self.backend, &mut self.records);
            let Some(record) = records[index].as_mut() else {
                return Ok(Mips4CachedBlockExecution::Missing);
            };
            if record.block.guard().lines().is_empty()
                || record.block.guard().code_source().is_some()
                || !cached_guard_valid(record, runtime)
            {
                (
                    Mips4CachedBlockExecution::Missing,
                    !record.block.guard().lines().is_empty()
                        && record.block.guard().code_source().is_none(),
                )
            } else if counters_dirty && record.counter_barrier {
                (Mips4CachedBlockExecution::CounterSynchronization, false)
            } else {
                frame.operations_executed = 0;
                frame.runtime_calls = 0;
                let (exit, tier) = if let Some(compiled) = &record.compiled {
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

                (
                    Mips4CachedBlockExecution::Executed(Mips4BlockExecution {
                        exit,
                        tier,
                        counter_barrier: record.counter_barrier,
                        operations_executed: frame.operations_executed,
                        runtime_calls: frame.runtime_calls,
                    }),
                    record.guard_mutating && !cached_guard_valid(record, runtime),
                )
            }
        };
        if invalidate {
            self.remove_dispatch_entry(key);
            let index = self
                .indices
                .remove(&key)
                .expect("a validated block index must remain present");
            let removed = self.records[index].take().is_some();
            debug_assert!(removed);
            self.free_records.push(index);
            self.statistics.invalidated_blocks =
                self.statistics.invalidated_blocks.saturating_add(1);
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
