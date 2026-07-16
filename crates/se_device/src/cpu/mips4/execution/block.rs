//! Portable typed basic-block IR, lifting, validation, and reference execution.

use core::fmt;
use se_core::scheduler::FractionalClockProjection;

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

/// Maximum number of guest instructions represented by one basic block.
pub const MIPS4_BLOCK_MAX_INSTRUCTIONS: usize = 32;

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

/// Machine-assigned identity of a stable external code source.
///
/// Identifiers are opaque to the CPU and must be unique within one execution
/// engine lifetime.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Mips4CodeSourceId(u64);

impl Mips4CodeSourceId {
    /// Creates a code-source identifier from a machine-assigned value.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the machine-assigned raw value.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Versioned identity of a side-effect-free external code window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4CodeGuard {
    /// Opaque external source identity.
    pub source_id: Mips4CodeSourceId,

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
        0x434f_4445_5352_4301
            ^ self.source_id.get().wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ self.source_offset.rotate_left(11)
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
    /// Returns the common synchronous outcome for a runtime operation.
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

/// Normalized CP0 operation used by shared runtime semantics.
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

/// Portable request passed to a fast-memory runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4FastMemoryReadRequest {
    physical_address: u64,
    retired_boundaries: u64,
    size: u32,
    kind: u32,
    access_type: u32,
}

impl Mips4FastMemoryReadRequest {
    /// Creates a portable request for one translated memory read.
    pub const fn new(
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

/// Result of one portable fast-memory read attempt.
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
    /// The runtime detected an invariant failure.
    InternalError,
}

/// Machine-owned runtime used for proven synchronous memory completion.
pub trait Mips4FastMemoryRuntime {
    /// Attempts one already translated, aligned read.
    fn read(&mut self, request: Mips4FastMemoryReadRequest) -> Mips4FastMemoryReadResult;

    /// Returns logical transactions completed since this runtime was created.
    fn completed_transactions(&self) -> u64 {
        0
    }
}

/// Shared runtime semantics invoked by translated execution.
pub trait Mips4BlockRuntime {
    /// Executes one normalized runtime operation without decoding it again.
    fn execute<F>(
        &mut self,
        frame: &mut Mips4BlockFrame,
        operation: Mips4RuntimeOperation,
        fast_memory: Option<&mut F>,
    ) -> Mips4RuntimeResult
    where
        F: Mips4FastMemoryRuntime + ?Sized;

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

    /// Execute one typed operation through shared runtime semantics.
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

    /// Returns normalized runtime operations in execution order.
    pub fn runtime_operations(&self) -> Vec<Mips4RuntimeOperation> {
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

/// Exception subset emitted directly by block execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4BlockException {
    /// Signed arithmetic overflow.
    ArithmeticOverflow,
    /// Misaligned instruction target.
    AddressErrorLoad,
    /// Integer trap condition.
    Trap,
    /// SYSCALL instruction.
    SystemCall,
    /// BREAK instruction.
    Breakpoint,
}

impl Mips4BlockException {
    /// Converts the block exception to the architecture exception.
    pub const fn architecture_exception(self) -> Mips4Exception {
        match self {
            Self::ArithmeticOverflow => Mips4Exception::ArithmeticOverflow,
            Self::AddressErrorLoad => Mips4Exception::AddressErrorLoad,
            Self::Trap => Mips4Exception::Trap,
            Self::SystemCall => Mips4Exception::Syscall,
            Self::Breakpoint => Mips4Exception::Breakpoint,
        }
    }
}

/// Architectural reason translated execution returned to the CPU dispatcher.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4BlockExit {
    /// The caller-provided retirement budget was exhausted.
    BudgetExhausted,
    /// The translated block completed and needs another dispatch.
    Dispatch,
    /// An architectural exception was recorded in the frame.
    Exception,
    /// The block guard became invalid before execution.
    GuardInvalid,
    /// A typed runtime operation started an asynchronous transaction.
    RuntimeTransaction,
    /// WAIT retired and left the processor in standby.
    RuntimeIdle,
    /// The slice timeline cannot admit the next instruction.
    TimelineExhausted,
    /// The execution implementation violated an internal invariant.
    InternalError,
}

/// Portable semantic state exported from a MIPS IV block frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mips4BlockFrameState {
    /// Integer register values, including an invariant zero register.
    pub gpr: [u64; MIPS4_GPR_COUNT],
    /// HI register value.
    pub hi: u64,
    /// LO register value.
    pub lo: u64,
    /// Current instruction address.
    pub pc: u64,
    /// Queued next instruction address.
    pub next_pc: u64,
    /// Branch address owning the current delay slot.
    pub delay_slot_branch_pc: Option<u64>,
    /// Remaining retirement budget.
    pub budget: u64,
    /// Normally retired instructions in the current slice.
    pub retired: u64,
    /// Recorded architectural exception.
    pub exception: Option<Mips4BlockException>,
    /// Guest operations entered by the current invocation.
    pub operations_executed: u64,
    /// Typed runtime helpers entered by the current invocation.
    pub runtime_calls: u64,
}

/// Portable semantic frame shared by MIPS IV execution implementations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mips4BlockFrame {
    gpr: [u64; MIPS4_GPR_COUNT],
    hi: u64,
    lo: u64,
    pc: u64,
    next_pc: u64,
    delay_slot_branch_pc: u64,
    delay_slot_valid: bool,
    budget: u64,
    retired: u64,
    exception: Option<Mips4BlockException>,
    operations_executed: u64,
    runtime_calls: u64,
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
            hi,
            lo,
            pc,
            next_pc,
            delay_slot_branch_pc: delay_slot_branch_pc.unwrap_or(0),
            delay_slot_valid: delay_slot_branch_pc.is_some(),
            budget,
            retired: 0,
            exception: None,
            operations_executed: 0,
            runtime_calls: 0,
        }
    }

    /// Creates a semantic frame from exported state.
    pub fn from_state(state: Mips4BlockFrameState) -> Self {
        let mut frame = Self::new(
            state.gpr,
            state.hi,
            state.lo,
            state.pc,
            state.next_pc,
            state.delay_slot_branch_pc,
            state.budget,
        );
        frame.retired = state.retired;
        frame.exception = state.exception;
        frame.operations_executed = state.operations_executed;
        frame.runtime_calls = state.runtime_calls;
        frame
    }

    /// Exports all portable semantic state without a layout commitment.
    pub const fn export_state(&self) -> Mips4BlockFrameState {
        Mips4BlockFrameState {
            gpr: self.gpr,
            hi: self.hi,
            lo: self.lo,
            pc: self.pc,
            next_pc: self.next_pc,
            delay_slot_branch_pc: self.delay_slot_branch_pc(),
            budget: self.budget,
            retired: self.retired,
            exception: self.exception,
            operations_executed: self.operations_executed,
            runtime_calls: self.runtime_calls,
        }
    }

    /// Imports all portable semantic state from an execution implementation.
    pub fn import_state(&mut self, mut state: Mips4BlockFrameState) {
        state.gpr[0] = 0;
        self.gpr = state.gpr;
        self.hi = state.hi;
        self.lo = state.lo;
        self.pc = state.pc;
        self.next_pc = state.next_pc;
        self.delay_slot_branch_pc = state.delay_slot_branch_pc.unwrap_or(0);
        self.delay_slot_valid = state.delay_slot_branch_pc.is_some();
        self.budget = state.budget;
        self.retired = state.retired;
        self.exception = state.exception;
        self.operations_executed = state.operations_executed;
        self.runtime_calls = state.runtime_calls;
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
        }
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
        if self.delay_slot_valid {
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

    /// Resets per-invocation accounting fields.
    pub fn reset_execution_accounting(&mut self) {
        self.operations_executed = 0;
        self.runtime_calls = 0;
    }

    /// Returns the recorded block exception.
    pub const fn exception(&self) -> Option<Mips4BlockException> {
        self.exception
    }

    /// Resets per-invocation budget and result fields.
    pub fn prepare(&mut self, budget: u64) {
        self.budget = budget;
        self.retired = 0;
        self.exception = None;
        self.operations_executed = 0;
        self.runtime_calls = 0;
        self.gpr[0] = 0;
    }

    /// Restricts the remaining retirement budget without increasing it.
    pub fn limit_budget(&mut self, budget: u64) {
        self.budget = self.budget.min(budget);
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
        self.delay_slot_valid = delay_slot_branch_pc.is_some();
    }
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
    struct RejectFastMemory;

    impl Mips4FastMemoryRuntime for RejectFastMemory {
        fn read(&mut self, _request: Mips4FastMemoryReadRequest) -> Mips4FastMemoryReadResult {
            Mips4FastMemoryReadResult::InternalError
        }
    }

    impl Mips4BlockRuntime for RejectRuntime {
        fn execute<F>(
            &mut self,
            _frame: &mut Mips4BlockFrame,
            _operation: Mips4RuntimeOperation,
            _fast_memory: Option<&mut F>,
        ) -> Mips4RuntimeResult
        where
            F: Mips4FastMemoryRuntime + ?Sized,
        {
            Mips4RuntimeResult::InternalError
        }
    }

    interpret_block_with_runtime::<_, RejectFastMemory>(block, frame, &mut RejectRuntime, None)
}

/// Executes a typed block with shared runtime semantics.
pub fn interpret_block_with_runtime<R, F>(
    block: &Mips4Block,
    frame: &mut Mips4BlockFrame,
    runtime: &mut R,
    mut fast_memory: Option<&mut F>,
) -> Mips4BlockExit
where
    R: Mips4BlockRuntime + ?Sized,
    F: Mips4FastMemoryRuntime + ?Sized,
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
        match interpret_operation(
            instruction.operation,
            frame,
            runtime,
            reborrow_fast_memory(&mut fast_memory),
        ) {
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
                frame.exception = Some(exception);
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
                frame.exception = Some(Mips4BlockException::AddressErrorLoad);
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
    match interpret_operation(
        delay_slot.operation,
        frame,
        runtime,
        reborrow_fast_memory(&mut fast_memory),
    ) {
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
            frame.exception = Some(exception);
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

fn reborrow_fast_memory<'a, F: ?Sized>(runtime: &'a mut Option<&mut F>) -> Option<&'a mut F> {
    runtime.as_mut().map(|runtime| &mut **runtime)
}

fn interpret_operation<R, F>(
    operation: Mips4BlockOperation,
    frame: &mut Mips4BlockFrame,
    runtime: &mut R,
    fast_memory: Option<&mut F>,
) -> Result<Mips4RuntimeResult, Mips4BlockException>
where
    R: Mips4BlockRuntime + ?Sized,
    F: Mips4FastMemoryRuntime + ?Sized,
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
            return Ok(runtime.execute(frame, operation, fast_memory));
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
    frame.delay_slot_valid = false;
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
        frame.delay_slot_valid = false;
        return;
    }
    frame.pc = frame.next_pc;
    frame.next_pc = if taken {
        target
    } else {
        frame.next_pc.wrapping_add(4)
    };
    frame.delay_slot_branch_pc = branch_pc;
    frame.delay_slot_valid = true;
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
    fn code_source_ids_are_opaque_and_separate_guard_tokens() {
        let first = Mips4CodeSourceId::new(7);
        let second = Mips4CodeSourceId::new(11);
        assert_eq!(first.get(), 7);
        assert_eq!(second.get(), 11);

        let guard = Mips4CodeGuard {
            source_id: first,
            source_offset: 0x120,
            revision: 3,
            fingerprint: 0x1234_5678_9abc_def0,
        };
        assert_ne!(
            guard.token(),
            Mips4CodeGuard {
                source_id: second,
                ..guard
            }
            .token()
        );
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
