//! Host-owned MIPS IV tiering, caches, profiling, and backend lifecycle.

use core::{cell::Cell, fmt};
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use se_device::cpu::mips4::cp1::decode::{Mips4Cp1Decode, Mips4Cp1InstructionClass};
use se_device::cpu::mips4::execution::block::*;
use se_device::cpu::mips4::execution::port::{
    Mips4BlockExecutionResult, Mips4BlockProbe, Mips4BlockSource, Mips4ExecutionPort,
    Mips4ReusableBatchExecution, Mips4ReusableBatchResult, Mips4ReusableBatchStop,
    Mips4ReusableBlockExecution,
};

use super::region::{Mips4Region, Mips4RegionNode, Mips4RegionSideExit};

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

/// Number of block entries before a native backend compiles a block.
pub const MIPS4_BLOCK_HOT_THRESHOLD: u64 = 64;
/// Maximum number of cached block records in one execution engine.
pub const MIPS4_BLOCK_CACHE_CAPACITY: usize = 16_384;
/// Guest operations observed at one entry before Region construction.
pub const MIPS4_REGION_HOT_THRESHOLD: u64 = 2_048;
/// Maximum number of unique block nodes in one Region.
pub const MIPS4_REGION_MAX_NODES: usize = 64;
/// Maximum number of unique guest operations in one Region.
pub const MIPS4_REGION_MAX_OPERATIONS: usize = 512;
/// Maximum number of derived Region records.
pub const MIPS4_REGION_CACHE_CAPACITY: usize = 4_096;

const MIPS4_REGION_MIN_SUCCESSOR_OBSERVATIONS: u64 = 256;
const MIPS4_REGION_DOMINANT_DIRECT_PERCENT: u64 = 75;
const MIPS4_REGION_DOMINANT_INDIRECT_PERCENT: u64 = 90;
pub(super) const MIPS4_REGION_MIN_ACYCLIC_OPERATIONS: usize = 16;
const MIPS4_REGION_RETRY_OPERATIONS: u64 = 65_536;
const MIPS4_BLOCK_DISPATCH_CACHE_CAPACITY: usize = MIPS4_BLOCK_CACHE_CAPACITY;

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

    /// Polls one baseline compilation without blocking.
    fn block_compilation_status(
        &mut self,
        _compiled: &Self::CompiledBlock,
        _wait: bool,
    ) -> Result<Mips4CompilationStatus, Self::Error> {
        Ok(Mips4CompilationStatus::ready())
    }

    /// Executes one compiled block against the stable frame ABI.
    fn execute<'fast, R>(
        &mut self,
        compiled: &Self::CompiledBlock,
        frame: &mut Mips4BlockFrame,
        runtime: &mut R,
        operations: &[Mips4RuntimeOperation],
        fast_memory: Option<&mut (dyn Mips4FastMemoryRuntime + 'fast)>,
    ) -> Result<Mips4BlockExit, Self::Error>
    where
        R: Mips4BlockRuntime;

    /// Compiles one verified bounded Region.
    fn compile_region(&mut self, region: &Mips4Region)
    -> Result<Self::CompiledRegion, Self::Error>;

    /// Polls one Region compilation without blocking.
    fn region_compilation_status(
        &mut self,
        _compiled: &Self::CompiledRegion,
        _wait: bool,
    ) -> Result<Mips4CompilationStatus, Self::Error> {
        Ok(Mips4CompilationStatus::ready())
    }

    /// Executes one compiled Region against the stable frame ABI.
    fn execute_region<'fast, R>(
        &mut self,
        compiled: &Self::CompiledRegion,
        frame: &mut Mips4BlockFrame,
        runtime: &mut R,
        operations: &[Mips4RuntimeOperation],
        fast_memory: Option<&mut (dyn Mips4FastMemoryRuntime + 'fast)>,
    ) -> Result<(Mips4BlockExit, Option<Mips4RegionSideExit>), Self::Error>
    where
        R: Mips4BlockRuntime;

    /// Invalidates all native code and schedules a backend allocation reset.
    fn clear(&mut self) -> Result<(), Self::Error>;
}

/// Availability and newly reported cost of one native compilation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4CompilationStatus {
    /// Whether the native entry point is ready to execute.
    pub ready: bool,

    /// Compilation nanoseconds reported since the previous poll.
    pub compilation_nanos: u64,
}

impl Mips4CompilationStatus {
    const fn ready() -> Self {
        Self {
            ready: true,
            compilation_nanos: 0,
        }
    }
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

    /// Fast-memory reads completed directly by native code.
    pub native_fast_memory_reads: u64,

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
    /// Reusable cached-dispatch batches that executed at least one entry.
    pub cached_dispatch_batches: u64,

    /// Reusable block or Region entries consumed inside cached-dispatch batches.
    pub cached_dispatch_entries: u64,

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

    /// Fast-memory reads completed directly by native code.
    pub native_fast_memory_reads: u64,

    /// Dynamically fetched instructions translated as single-instruction blocks.
    pub dynamic_fetches: u64,

    /// Instructions fetched through a stable external code window.
    pub fast_fetches: u64,

    /// Blocks compiled by the native backend.
    pub compiled_blocks: u64,

    /// Host nanoseconds spent compiling baseline blocks.
    pub block_compile_nanos: u64,

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

    /// Host nanoseconds spent lifting profiled Regions.
    pub region_lifting_nanos: u64,

    /// Host nanoseconds spent compiling Regions.
    pub region_compile_nanos: u64,

    /// Region exits caused by an uncompiled successor edge.
    pub region_cold_side_exits: u64,

    /// Region exits caused by the retirement budget.
    pub region_budget_side_exits: u64,

    /// Region exits caused by a typed runtime operation.
    pub region_runtime_side_exits: u64,

    /// Region entries rejected by a visibility or execution guard.
    pub region_guard_side_exits: u64,
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
pub(super) enum Mips4RegionSource {
    InstructionCache,
    Stable(Mips4CodeSourceId),
}

pub(super) fn region_source_kind(guard: &Mips4BlockGuard) -> Option<Mips4RegionSource> {
    match (guard.lines().is_empty(), guard.code_source()) {
        (false, None) => Some(Mips4RegionSource::InstructionCache),
        (true, Some(source)) => Some(Mips4RegionSource::Stable(source.source_id)),
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

fn profiled_region_successors<C, R>(record: &Mips4BlockRecord<C, R>) -> Vec<Mips4BlockKey> {
    if record
        .block
        .branch()
        .is_some_and(|branch| matches!(branch.target, Mips4BlockBranchTarget::Register(_)))
    {
        return dominant_successor(record).into_iter().collect();
    }

    let mut successors = record
        .successors
        .iter()
        .filter(|profile| profile.key.is_some() && profile.observations != 0)
        .copied()
        .collect::<Vec<_>>();
    successors.sort_unstable_by_key(|successor| core::cmp::Reverse(successor.observations));
    successors
        .into_iter()
        .filter_map(|profile| profile.key)
        .collect()
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
    let mut nodes = vec![Mips4RegionNode::new(entry_record.block.clone(), None)];
    let mut node_keys = vec![entry_key];
    let mut node_records = vec![entry_index];
    let mut operation_count = entry_record.block.instruction_count();
    let mut current_node = 0_usize;

    while current_node < nodes.len() {
        let record = records.get(node_records[current_node])?.as_ref()?;
        let mut region_successors = Vec::new();
        for successor in profiled_region_successors(record) {
            if let Some(successor_node) = node_keys.iter().position(|key| *key == successor) {
                region_successors.push(successor_node);
                continue;
            }
            if nodes.len() == MIPS4_REGION_MAX_NODES {
                continue;
            }
            let Some(successor_index) = indices.get(&successor).copied() else {
                continue;
            };
            let Some(successor_record) = records.get(successor_index).and_then(Option::as_ref)
            else {
                continue;
            };
            let successor_key = successor_record.block.key();
            let successor_operations = successor_record.block.instruction_count();
            if !region_record_executable(successor_record)
                || successor_key.fetch_context != entry_key.fetch_context
                || successor_key.translation_generation != entry_key.translation_generation
                || successor_key.code_guard != entry_key.code_guard
                || region_source_kind(successor_record.block.guard()) != Some(entry_source)
                || operation_count.saturating_add(successor_operations)
                    > MIPS4_REGION_MAX_OPERATIONS
            {
                continue;
            }
            let successor_node = nodes.len();
            operation_count = operation_count.saturating_add(successor_operations);
            node_keys.push(successor_key);
            node_records.push(successor_index);
            nodes.push(Mips4RegionNode::new(successor_record.block.clone(), None));
            region_successors.push(successor_node);
        }
        nodes[current_node].set_successors(region_successors);
        current_node += 1;
    }

    Mips4Region::new(nodes).ok()
}

struct Mips4CompiledRegionRecord<R> {
    compiled: R,
    runtime_operations: Vec<Mips4RuntimeOperation>,
    member_keys: Vec<Mips4BlockKey>,
    guards: Vec<Mips4BlockGuard>,
    guard_validation_epoch: Option<u64>,
}

struct Mips4BlockRecord<C, R> {
    block: Mips4Block,
    runtime_operations: Vec<Mips4RuntimeOperation>,
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
    R: Mips4BlockRuntime + ?Sized,
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
fn execute_interpreted_tier<'fast, B, R>(
    backend: &mut B,
    record: &mut Mips4BlockRecord<B::CompiledBlock, B::CompiledRegion>,
    frame: &mut Mips4BlockFrame,
    runtime: &mut R,
    fast_memory: Option<&mut (dyn Mips4FastMemoryRuntime + 'fast)>,
) -> Result<(Mips4BlockExit, bool, u64), Mips4BlockEngineError<B::Error>>
where
    B: Mips4CodegenBackend,
    R: Mips4BlockRuntime,
{
    let exit = interpret_block_with_runtime(&record.block, frame, runtime, fast_memory);
    record.operation_hotness = record
        .operation_hotness
        .saturating_add(frame.operations_executed());
    let promoted =
        record.compiled.is_none() && record.operation_hotness >= MIPS4_BLOCK_HOT_THRESHOLD;
    let mut compile_nanos = 0;
    if promoted {
        let started = std::time::Instant::now();
        record.compiled = Some(
            backend
                .compile(&record.block)
                .map_err(Mips4BlockEngineError::Backend)?,
        );
        compile_nanos = duration_nanos(started.elapsed());
    }
    Ok((exit, promoted, compile_nanos))
}

fn duration_nanos(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
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
            self.remove_dispatch_entry(key);
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
    pub fn record_fast_fetches(&mut self, instructions: u64) {
        self.statistics.fast_fetches = self.statistics.fast_fetches.saturating_add(instructions);
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
        for record in self.records.iter_mut() {
            let Some(record) = record.as_mut() else {
                continue;
            };
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
        let lifting_started = std::time::Instant::now();
        let region = build_profiled_region(entry_index, &self.indices, &self.records);
        self.statistics.region_lifting_nanos = self
            .statistics
            .region_lifting_nanos
            .saturating_add(duration_nanos(lifting_started.elapsed()));
        let Some(region) = region else {
            if let Some(record) = self.records.get_mut(entry_index).and_then(Option::as_mut) {
                record.region_next_compile_hotness = record
                    .region_hotness
                    .saturating_add(MIPS4_REGION_RETRY_OPERATIONS);
            }
            return Ok(());
        };
        let runtime_operations = region.runtime_operations();
        let member_keys = region.member_keys();
        let guards = region.guards();
        let entry_key = region.key().entry;
        let compile_started = std::time::Instant::now();
        let compiled = self
            .backend
            .compile_region(&region)
            .map_err(Mips4BlockEngineError::Backend)?;
        self.statistics.region_compile_nanos = self
            .statistics
            .region_compile_nanos
            .saturating_add(duration_nanos(compile_started.elapsed()));
        let Some(record) = self.records.get_mut(entry_index).and_then(Option::as_mut) else {
            return Err(Mips4BlockEngineError::MissingBlock(Box::new(entry_key)));
        };
        if record.block.key() != entry_key || record.compiled_region.is_some() {
            return Ok(());
        }
        record.compiled_region = Some(Mips4CompiledRegionRecord {
            compiled,
            runtime_operations,
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

        self.execute_with_runtime(key, frame, &mut RejectRuntime, None)
    }

    /// Executes one cached block with access to typed shared runtime semantics.
    pub fn execute_with_runtime<'fast, R>(
        &mut self,
        key: Mips4BlockKey,
        frame: &mut Mips4BlockFrame,
        runtime: &mut R,
        fast_memory: Option<&mut (dyn Mips4FastMemoryRuntime + 'fast)>,
    ) -> Result<Mips4BlockExecution, Mips4BlockEngineError<B::Error>>
    where
        R: Mips4BlockRuntime,
    {
        let index = self
            .record_index(key)
            .ok_or_else(|| Mips4BlockEngineError::MissingBlock(Box::new(key)))?;
        frame.reset_execution_accounting();
        let (exit, tier, counter_barrier, promoted, mut region_side_exit) = {
            let (backend, records) = (&mut self.backend, &mut self.records);
            let record = records[index]
                .as_mut()
                .ok_or_else(|| Mips4BlockEngineError::MissingBlock(Box::new(key)))?;
            let region_status = record
                .compiled_region
                .as_ref()
                .map(|region| backend.region_compilation_status(&region.compiled, false))
                .transpose()
                .map_err(Mips4BlockEngineError::Backend)?;
            let block_status = record
                .compiled
                .as_ref()
                .map(|compiled| backend.block_compilation_status(compiled, false))
                .transpose()
                .map_err(Mips4BlockEngineError::Backend)?;
            self.statistics.region_compile_nanos = self
                .statistics
                .region_compile_nanos
                .saturating_add(region_status.map_or(0, |status| status.compilation_nanos));
            self.statistics.block_compile_nanos = self
                .statistics
                .block_compile_nanos
                .saturating_add(block_status.map_or(0, |status| status.compilation_nanos));
            if region_status.is_some_and(|status| status.ready) {
                let region = record
                    .compiled_region
                    .as_ref()
                    .expect("a ready Region status requires a compiled record");
                let (exit, region_side_exit) = backend
                    .execute_region(
                        &region.compiled,
                        frame,
                        runtime,
                        &region.runtime_operations,
                        fast_memory,
                    )
                    .map_err(Mips4BlockEngineError::Backend)?;
                (
                    exit,
                    Mips4BlockTier::Region,
                    record.counter_barrier,
                    false,
                    region_side_exit,
                )
            } else if block_status.is_some_and(|status| status.ready) {
                let compiled = record
                    .compiled
                    .as_ref()
                    .expect("a ready block status requires a compiled record");
                let exit = backend
                    .execute(
                        compiled,
                        frame,
                        runtime,
                        &record.runtime_operations,
                        fast_memory,
                    )
                    .map_err(Mips4BlockEngineError::Backend)?;
                (
                    exit,
                    Mips4BlockTier::Native,
                    record.counter_barrier,
                    false,
                    None,
                )
            } else {
                let (exit, promoted, compile_nanos) =
                    execute_interpreted_tier(backend, record, frame, runtime, fast_memory)?;
                self.statistics.block_compile_nanos = self
                    .statistics
                    .block_compile_nanos
                    .saturating_add(compile_nanos);
                (
                    exit,
                    Mips4BlockTier::Interpreter,
                    record.counter_barrier,
                    promoted,
                    None,
                )
            }
        };

        if tier == Mips4BlockTier::Region && exit == Mips4BlockExit::BudgetExhausted {
            region_side_exit = Some(Mips4RegionSideExit::Budget);
        }

        match tier {
            Mips4BlockTier::Interpreter => {
                self.statistics.interpreted_blocks =
                    self.statistics.interpreted_blocks.saturating_add(1);
                self.statistics.interpreted_operations = self
                    .statistics
                    .interpreted_operations
                    .saturating_add(frame.operations_executed());
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
                    .saturating_add(frame.operations_executed());
            }
            Mips4BlockTier::Region => {
                self.statistics.region_entries = self.statistics.region_entries.saturating_add(1);
                self.statistics.region_operations = self
                    .statistics
                    .region_operations
                    .saturating_add(frame.operations_executed());
                record_region_side_exit(&mut self.statistics, region_side_exit);
            }
        }
        self.statistics.runtime_calls = self
            .statistics
            .runtime_calls
            .saturating_add(frame.runtime_calls());
        self.statistics.native_fast_memory_reads = self
            .statistics
            .native_fast_memory_reads
            .saturating_add(frame.native_fast_memory_reads());

        if tier != Mips4BlockTier::Region {
            let record = self.records[index]
                .as_mut()
                .ok_or_else(|| Mips4BlockEngineError::MissingBlock(Box::new(key)))?;
            record.region_hotness = record
                .region_hotness
                .saturating_add(frame.operations_executed());
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
            operations_executed: frame.operations_executed(),
            runtime_calls: frame.runtime_calls(),
            native_fast_memory_reads: frame.native_fast_memory_reads(),
            region_side_exit,
        })
    }

    /// Executes one reusable I-cache block with a single record lookup.
    pub fn execute_cached_with_runtime<'fast, R>(
        &mut self,
        key: Mips4BlockKey,
        frame: &mut Mips4BlockFrame,
        runtime: &mut R,
        fast_memory: Option<&mut (dyn Mips4FastMemoryRuntime + 'fast)>,
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
                }
                (Mips4CachedBlockExecution::Missing, reusable_source)
            } else if counters_dirty && record.counter_barrier {
                (Mips4CachedBlockExecution::CounterSynchronization, false)
            } else {
                frame.reset_execution_accounting();
                let region_status = record
                    .compiled_region
                    .as_ref()
                    .map(|region| backend.region_compilation_status(&region.compiled, false))
                    .transpose()
                    .map_err(Mips4BlockEngineError::Backend)?;
                let block_status = record
                    .compiled
                    .as_ref()
                    .map(|compiled| backend.block_compilation_status(compiled, false))
                    .transpose()
                    .map_err(Mips4BlockEngineError::Backend)?;
                self.statistics.region_compile_nanos = self
                    .statistics
                    .region_compile_nanos
                    .saturating_add(region_status.map_or(0, |status| status.compilation_nanos));
                self.statistics.block_compile_nanos = self
                    .statistics
                    .block_compile_nanos
                    .saturating_add(block_status.map_or(0, |status| status.compilation_nanos));
                let (exit, tier, mut region_side_exit) =
                    if region_status.is_some_and(|status| status.ready) {
                        let region = record
                            .compiled_region
                            .as_ref()
                            .expect("a ready Region status requires a compiled record");
                        let (exit, region_side_exit) = backend
                            .execute_region(
                                &region.compiled,
                                frame,
                                runtime,
                                &region.runtime_operations,
                                fast_memory,
                            )
                            .map_err(Mips4BlockEngineError::Backend)?;
                        (exit, Mips4BlockTier::Region, region_side_exit)
                    } else if block_status.is_some_and(|status| status.ready) {
                        let compiled = record
                            .compiled
                            .as_ref()
                            .expect("a ready block status requires a compiled record");
                        let exit = backend
                            .execute(
                                compiled,
                                frame,
                                runtime,
                                &record.runtime_operations,
                                fast_memory,
                            )
                            .map_err(Mips4BlockEngineError::Backend)?;
                        (exit, Mips4BlockTier::Native, None)
                    } else {
                        let (exit, promoted, compile_nanos) =
                            execute_interpreted_tier(backend, record, frame, runtime, fast_memory)?;
                        self.statistics.block_compile_nanos = self
                            .statistics
                            .block_compile_nanos
                            .saturating_add(compile_nanos);
                        if promoted {
                            self.statistics.compiled_blocks =
                                self.statistics.compiled_blocks.saturating_add(1);
                        }
                        (exit, Mips4BlockTier::Interpreter, None)
                    };

                record.region_hotness = record
                    .region_hotness
                    .saturating_add(frame.operations_executed());
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
                    region_side_exit = Some(Mips4RegionSideExit::Budget);
                }

                (
                    Mips4CachedBlockExecution::Executed(Mips4BlockExecution {
                        exit,
                        tier,
                        counter_barrier: record.counter_barrier,
                        operations_executed: frame.operations_executed(),
                        runtime_calls: frame.runtime_calls(),
                        native_fast_memory_reads: frame.native_fast_memory_reads(),
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
        R: Mips4BlockRuntime + ?Sized,
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

    fn record_reusable_execution_statistics(&mut self, execution: Mips4BlockExecution) {
        let mut statistics = Mips4BlockEngineStatistics::default();
        match execution.tier {
            Mips4BlockTier::Interpreter => {
                statistics.interpreted_blocks = 1;
                statistics.interpreted_operations = execution.operations_executed;
            }
            Mips4BlockTier::Native => {
                statistics.native_blocks = 1;
                statistics.native_operations = execution.operations_executed;
            }
            Mips4BlockTier::Region => {
                statistics.region_entries = 1;
                statistics.region_operations = execution.operations_executed;
                record_region_side_exit(&mut statistics, execution.region_side_exit);
            }
        }
        statistics.runtime_calls = execution.runtime_calls;
        statistics.native_fast_memory_reads = execution.native_fast_memory_reads;
        self.record_cached_statistics(statistics);
    }

    fn finish_reusable_batch(
        &mut self,
        execution: Mips4BlockExecutionResult,
        stop: Mips4ReusableBatchStop,
        entries: u64,
    ) -> Mips4ReusableBatchExecution {
        self.statistics.cached_dispatch_batches =
            self.statistics.cached_dispatch_batches.saturating_add(1);
        self.statistics.cached_dispatch_entries = self
            .statistics
            .cached_dispatch_entries
            .saturating_add(entries);
        Mips4ReusableBatchExecution::Executed(Mips4ReusableBatchResult {
            execution,
            stop,
            entries,
        })
    }

    /// Commits execution counters accumulated by a cached-block dispatcher.
    pub fn record_cached_statistics(&mut self, statistics: Mips4BlockEngineStatistics) {
        self.statistics.cached_dispatch_batches = self
            .statistics
            .cached_dispatch_batches
            .saturating_add(statistics.cached_dispatch_batches);
        self.statistics.cached_dispatch_entries = self
            .statistics
            .cached_dispatch_entries
            .saturating_add(statistics.cached_dispatch_entries);
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
        self.statistics.native_fast_memory_reads = self
            .statistics
            .native_fast_memory_reads
            .saturating_add(statistics.native_fast_memory_reads);
        self.statistics.block_compile_nanos = self
            .statistics
            .block_compile_nanos
            .saturating_add(statistics.block_compile_nanos);
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
        self.statistics.region_lifting_nanos = self
            .statistics
            .region_lifting_nanos
            .saturating_add(statistics.region_lifting_nanos);
        self.statistics.region_compile_nanos = self
            .statistics
            .region_compile_nanos
            .saturating_add(statistics.region_compile_nanos);
    }

    /// Waits for every requested native compilation and records its host cost.
    pub fn finish_compilations(&mut self) -> Result<(), Mips4BlockEngineError<B::Error>> {
        let mut block_compile_nanos = 0_u64;
        let mut region_compile_nanos = 0_u64;
        for record in self.records.iter().filter_map(Option::as_ref) {
            if let Some(compiled) = &record.compiled {
                let status = self
                    .backend
                    .block_compilation_status(compiled, true)
                    .map_err(Mips4BlockEngineError::Backend)?;
                debug_assert!(status.ready);
                block_compile_nanos = block_compile_nanos.saturating_add(status.compilation_nanos);
            }
            if let Some(region) = &record.compiled_region {
                let status = self
                    .backend
                    .region_compilation_status(&region.compiled, true)
                    .map_err(Mips4BlockEngineError::Backend)?;
                debug_assert!(status.ready);
                region_compile_nanos =
                    region_compile_nanos.saturating_add(status.compilation_nanos);
            }
        }
        self.statistics.block_compile_nanos = self
            .statistics
            .block_compile_nanos
            .saturating_add(block_compile_nanos);
        self.statistics.region_compile_nanos = self
            .statistics
            .region_compile_nanos
            .saturating_add(region_compile_nanos);
        Ok(())
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

    /// A block guard does not match its declared source.
    SourceMismatch {
        /// Rejected block identity.
        key: Box<Mips4BlockKey>,
        /// Declared instruction source.
        source: Mips4BlockSource,
    },
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
            Self::SourceMismatch { key, source } => {
                write!(
                    formatter,
                    "MIPS IV block {key:?} does not match source {source:?}"
                )
            }
        }
    }
}

impl<E> std::error::Error for Mips4BlockEngineError<E> where E: std::error::Error + 'static {}

impl<B> Mips4ExecutionPort for Mips4BlockEngine<B>
where
    B: Mips4CodegenBackend,
    B::Error: fmt::Display,
{
    type Error = Mips4BlockEngineError<B::Error>;
    type FastMemoryRuntime = dyn Mips4FastMemoryRuntime;

    fn probe<R>(
        &mut self,
        key: Mips4BlockKey,
        source: Mips4BlockSource,
        runtime: &R,
    ) -> Mips4BlockProbe
    where
        R: Mips4BlockRuntime + ?Sized,
    {
        let source_matches = self.block(key).is_some_and(|block| match source {
            Mips4BlockSource::InstructionCache => {
                !block.guard().lines().is_empty() && block.guard().code_source().is_none()
            }
            Mips4BlockSource::DynamicFetch => {
                block.guard().lines().is_empty() && block.guard().code_source().is_none()
            }
            Mips4BlockSource::Stable(guard) => block.guard().code_source() == Some(guard),
        });
        let ready = match source {
            Mips4BlockSource::InstructionCache => {
                self.reusable_instruction_cache_block(key, runtime)
            }
            Mips4BlockSource::DynamicFetch => self.block(key).is_some_and(|block| {
                block.guard().lines().is_empty()
                    && block.guard().code_source().is_none()
                    && runtime.block_guard_valid(block.guard())
            }),
            Mips4BlockSource::Stable(guard) => self.block(key).is_some_and(|block| {
                block.guard().code_source() == Some(guard)
                    && runtime.block_guard_valid(block.guard())
            }),
        };
        if ready {
            Mips4BlockProbe::Ready {
                counter_barrier: self.counter_barrier(key).unwrap_or(false),
            }
        } else {
            if source_matches {
                self.invalidate(key);
            }
            Mips4BlockProbe::Missing
        }
    }

    fn install(&mut self, block: Mips4Block, source: Mips4BlockSource) -> Result<(), Self::Error> {
        let guard = block.guard();
        let valid = match source {
            Mips4BlockSource::InstructionCache => {
                !guard.lines().is_empty() && guard.code_source().is_none()
            }
            Mips4BlockSource::DynamicFetch => {
                guard.lines().is_empty() && guard.code_source().is_none()
            }
            Mips4BlockSource::Stable(expected) => guard.code_source() == Some(expected),
        };
        if !valid {
            return Err(Mips4BlockEngineError::SourceMismatch {
                key: Box::new(block.key()),
                source,
            });
        }
        if source == Mips4BlockSource::DynamicFetch {
            self.insert_dynamic(block)
        } else {
            self.insert(block)
        }
    }

    fn execute<R>(
        &mut self,
        key: Mips4BlockKey,
        frame: &mut Mips4BlockFrame,
        runtime: &mut R,
        fast_memory: Option<&mut Self::FastMemoryRuntime>,
    ) -> Result<Mips4BlockExecutionResult, Self::Error>
    where
        R: Mips4BlockRuntime,
    {
        let stable_source = self
            .block(key)
            .and_then(|block| block.guard().code_source())
            .is_some();
        let execution = self.execute_with_runtime(key, frame, runtime, fast_memory)?;
        if stable_source {
            self.record_fast_fetches(execution.operations_executed);
        }
        if execution.exit == Mips4BlockExit::GuardInvalid
            || self
                .block(key)
                .is_some_and(|block| !runtime.block_guard_valid(block.guard()))
        {
            self.invalidate(key);
        }
        Ok(Mips4BlockExecutionResult {
            exit: execution.exit,
            counter_barrier: execution.counter_barrier,
            operations_executed: execution.operations_executed,
        })
    }

    fn execute_reusable<R>(
        &mut self,
        key: Mips4BlockKey,
        frame: &mut Mips4BlockFrame,
        runtime: &mut R,
        fast_memory: Option<&mut Self::FastMemoryRuntime>,
        counters_dirty: bool,
    ) -> Result<Mips4ReusableBlockExecution, Self::Error>
    where
        R: Mips4BlockRuntime,
    {
        match self.execute_cached_with_runtime(key, frame, runtime, fast_memory, counters_dirty)? {
            Mips4CachedBlockExecution::Missing => Ok(Mips4ReusableBlockExecution::Missing),
            Mips4CachedBlockExecution::CounterSynchronization => {
                Ok(Mips4ReusableBlockExecution::CounterSynchronization)
            }
            Mips4CachedBlockExecution::Executed(execution) => {
                self.record_reusable_execution_statistics(execution);
                Ok(Mips4ReusableBlockExecution::Executed(
                    Mips4BlockExecutionResult {
                        exit: execution.exit,
                        counter_barrier: execution.counter_barrier,
                        operations_executed: execution.operations_executed,
                    },
                ))
            }
        }
    }

    fn execute_reusable_batch<R>(
        &mut self,
        key: Mips4BlockKey,
        frame: &mut Mips4BlockFrame,
        runtime: &mut R,
        fast_memory: Option<&mut Self::FastMemoryRuntime>,
        counters_dirty: bool,
    ) -> Result<Mips4ReusableBatchExecution, Self::Error>
    where
        R: Mips4BlockRuntime,
    {
        let mut key = key;
        let mut fast_memory = fast_memory;
        let mut counters_dirty = counters_dirty;
        let mut entries = 0_u64;
        let mut operations_executed = 0_u64;
        let mut last_execution = None;

        loop {
            let previous_retired = frame.retired();
            let execution = self.execute_cached_with_runtime(
                key,
                frame,
                runtime,
                fast_memory.as_deref_mut(),
                counters_dirty,
            )?;
            let execution = match execution {
                Mips4CachedBlockExecution::Missing => {
                    let Some(execution) = last_execution else {
                        return Ok(Mips4ReusableBatchExecution::Missing);
                    };
                    return Ok(self.finish_reusable_batch(
                        execution,
                        Mips4ReusableBatchStop::MissingSuccessor,
                        entries,
                    ));
                }
                Mips4CachedBlockExecution::CounterSynchronization => {
                    let Some(execution) = last_execution else {
                        return Ok(Mips4ReusableBatchExecution::CounterSynchronization);
                    };
                    return Ok(self.finish_reusable_batch(
                        execution,
                        Mips4ReusableBatchStop::CounterSynchronization,
                        entries,
                    ));
                }
                Mips4CachedBlockExecution::Executed(execution) => execution,
            };

            self.record_reusable_execution_statistics(execution);
            entries = entries.saturating_add(1);
            operations_executed = operations_executed.saturating_add(execution.operations_executed);
            let made_retirement = frame.retired() != previous_retired;
            counters_dirty |= made_retirement;
            let aggregate = Mips4BlockExecutionResult {
                exit: execution.exit,
                counter_barrier: execution.counter_barrier,
                operations_executed,
            };
            last_execution = Some(aggregate);

            if execution.exit != Mips4BlockExit::Dispatch
                || execution.counter_barrier
                || frame.budget() == 0
                || !made_retirement
            {
                return Ok(self.finish_reusable_batch(
                    aggregate,
                    Mips4ReusableBatchStop::BlockExit,
                    entries,
                ));
            }

            key.pc = frame.pc();
            key.next_pc = frame.next_pc();
            key.delay_slot_branch_pc = frame.delay_slot_branch_pc();
        }
    }
}

#[cfg(test)]
mod tests {
    use core::convert::Infallible;

    use super::*;
    struct TestBackend;

    impl Mips4CodegenBackend for TestBackend {
        type CompiledBlock = Mips4Block;
        type CompiledRegion = Mips4Region;
        type Error = Infallible;

        fn compile(&mut self, block: &Mips4Block) -> Result<Self::CompiledBlock, Self::Error> {
            Ok(block.clone())
        }

        fn execute<'fast, R>(
            &mut self,
            compiled: &Self::CompiledBlock,
            frame: &mut Mips4BlockFrame,
            runtime: &mut R,
            _operations: &[Mips4RuntimeOperation],
            fast_memory: Option<&mut (dyn Mips4FastMemoryRuntime + 'fast)>,
        ) -> Result<Mips4BlockExit, Self::Error>
        where
            R: Mips4BlockRuntime,
        {
            Ok(interpret_block_with_runtime(
                compiled,
                frame,
                runtime,
                fast_memory,
            ))
        }

        fn compile_region(
            &mut self,
            region: &Mips4Region,
        ) -> Result<Self::CompiledRegion, Self::Error> {
            Ok(region.clone())
        }

        fn execute_region<'fast, R>(
            &mut self,
            _compiled: &Self::CompiledRegion,
            _frame: &mut Mips4BlockFrame,
            _runtime: &mut R,
            _operations: &[Mips4RuntimeOperation],
            _fast_memory: Option<&mut (dyn Mips4FastMemoryRuntime + 'fast)>,
        ) -> Result<(Mips4BlockExit, Option<Mips4RegionSideExit>), Self::Error>
        where
            R: Mips4BlockRuntime,
        {
            Ok((Mips4BlockExit::InternalError, None))
        }

        fn clear(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct PendingBackend {
        polls: usize,
    }

    impl Mips4CodegenBackend for PendingBackend {
        type CompiledBlock = Mips4Block;
        type CompiledRegion = Mips4Region;
        type Error = Infallible;

        fn compile(&mut self, block: &Mips4Block) -> Result<Self::CompiledBlock, Self::Error> {
            Ok(block.clone())
        }

        fn block_compilation_status(
            &mut self,
            _compiled: &Self::CompiledBlock,
            wait: bool,
        ) -> Result<Mips4CompilationStatus, Self::Error> {
            assert!(!wait, "the execution path waited for native compilation");
            self.polls += 1;
            Ok(Mips4CompilationStatus {
                ready: false,
                compilation_nanos: 0,
            })
        }

        fn execute<'fast, R>(
            &mut self,
            _compiled: &Self::CompiledBlock,
            _frame: &mut Mips4BlockFrame,
            _runtime: &mut R,
            _operations: &[Mips4RuntimeOperation],
            _fast_memory: Option<&mut (dyn Mips4FastMemoryRuntime + 'fast)>,
        ) -> Result<Mips4BlockExit, Self::Error>
        where
            R: Mips4BlockRuntime,
        {
            unreachable!("a pending compilation cannot execute")
        }

        fn compile_region(
            &mut self,
            region: &Mips4Region,
        ) -> Result<Self::CompiledRegion, Self::Error> {
            Ok(region.clone())
        }

        fn execute_region<'fast, R>(
            &mut self,
            _compiled: &Self::CompiledRegion,
            _frame: &mut Mips4BlockFrame,
            _runtime: &mut R,
            _operations: &[Mips4RuntimeOperation],
            _fast_memory: Option<&mut (dyn Mips4FastMemoryRuntime + 'fast)>,
        ) -> Result<(Mips4BlockExit, Option<Mips4RegionSideExit>), Self::Error>
        where
            R: Mips4BlockRuntime,
        {
            unreachable!("a pending Region compilation cannot execute")
        }

        fn clear(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    struct TestRuntime {
        guard_valid: bool,
        epoch: u64,
    }

    impl Mips4BlockRuntime for TestRuntime {
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

        fn block_guard_valid(&self, _guard: &Mips4BlockGuard) -> bool {
            self.guard_valid
        }

        fn block_guard_epoch(&self) -> u64 {
            self.epoch
        }
    }

    fn code_guard() -> Mips4CodeGuard {
        Mips4CodeGuard {
            source_id: Mips4CodeSourceId::new(1),
            source_offset: 0,
            revision: 1,
            fingerprint: 2,
        }
    }

    fn block(pc: u64, source: Mips4BlockSource) -> Mips4Block {
        let mut guard = match source {
            Mips4BlockSource::InstructionCache => {
                let mut guard = Mips4BlockGuard::new();
                guard.insert(Mips4BlockGuardLine {
                    set: 0,
                    way: 0,
                    physical_line_base: pc & !31,
                    generation: 1,
                });
                guard
            }
            Mips4BlockSource::DynamicFetch => Mips4BlockGuard::new(),
            Mips4BlockSource::Stable(guard) => Mips4BlockGuard::from_code_source(guard),
        };
        if source == Mips4BlockSource::InstructionCache && guard.lines().is_empty() {
            unreachable!();
        }
        let key = Mips4BlockKey {
            pc,
            next_pc: pc + 4,
            delay_slot_branch_pc: None,
            fetch_context: 0,
            translation_generation: 0,
            code_guard: match source {
                Mips4BlockSource::Stable(guard) => guard.token(),
                Mips4BlockSource::InstructionCache | Mips4BlockSource::DynamicFetch => 0,
            },
        };
        let mut block = Mips4Block::new(key, core::mem::take(&mut guard));
        block
            .push(Mips4BlockInstruction {
                metadata: Mips4BlockInstructionMetadata {
                    pc,
                    instruction: 0,
                    delay_slot_branch_pc: None,
                },
                operation: Mips4BlockOperation::NoOperation,
                retire: Mips4BlockRetire { pc },
            })
            .unwrap();
        block.terminate_dispatch().unwrap();
        block
    }

    fn direct_self_loop_block(pc: u64) -> Mips4Block {
        let key = Mips4BlockKey {
            pc,
            next_pc: pc + 4,
            delay_slot_branch_pc: None,
            fetch_context: 0,
            translation_generation: 0,
            code_guard: 0,
        };
        let mut guard = Mips4BlockGuard::new();
        guard.insert(Mips4BlockGuardLine {
            set: 0,
            way: 0,
            physical_line_base: pc & !31,
            generation: 1,
        });
        let mut block = Mips4Block::new(key, guard);
        block
            .terminate_with_branch(
                Mips4BlockBranch {
                    metadata: Mips4BlockInstructionMetadata {
                        pc,
                        instruction: 0,
                        delay_slot_branch_pc: None,
                    },
                    condition: Mips4BlockBranchCondition::Always,
                    target: Mips4BlockBranchTarget::Direct(pc),
                    likely: false,
                    link: None,
                    retire: Mips4BlockRetire { pc },
                },
                Mips4BlockInstruction {
                    metadata: Mips4BlockInstructionMetadata {
                        pc: pc + 4,
                        instruction: 0,
                        delay_slot_branch_pc: Some(pc),
                    },
                    operation: Mips4BlockOperation::NoOperation,
                    retire: Mips4BlockRetire { pc: pc + 4 },
                },
            )
            .unwrap();
        block
    }

    fn frame(key: Mips4BlockKey) -> Mips4BlockFrame {
        Mips4BlockFrame::new([0; 32], 0, 0, key.pc, key.next_pc, None, 1)
    }

    #[test]
    fn threshold_promotion_preserves_interpreter_and_native_statistics() {
        let source = Mips4BlockSource::DynamicFetch;
        let block = block(0x1000, source);
        let key = block.key();
        let mut engine = Mips4BlockEngine::new(TestBackend);
        engine.install(block, source).unwrap();
        let mut runtime = TestRuntime {
            guard_valid: true,
            epoch: 0,
        };
        for _ in 0..=MIPS4_BLOCK_HOT_THRESHOLD {
            let mut frame = frame(key);
            Mips4ExecutionPort::execute(&mut engine, key, &mut frame, &mut runtime, None).unwrap();
        }
        let statistics = engine.statistics();
        assert_eq!(statistics.compiled_blocks, 1);
        assert_eq!(statistics.interpreted_blocks, MIPS4_BLOCK_HOT_THRESHOLD);
        assert_eq!(statistics.native_blocks, 1);
        assert_eq!(statistics.interpreted_operations, MIPS4_BLOCK_HOT_THRESHOLD);
        assert_eq!(statistics.native_operations, 1);
        assert_eq!(statistics.dynamic_fetches, 1);
    }

    #[test]
    fn pending_self_loop_compilation_never_blocks_execution_entries() {
        let source = Mips4BlockSource::InstructionCache;
        let block = direct_self_loop_block(0x1000);
        let key = block.key();
        let mut engine = Mips4BlockEngine::new(PendingBackend::default());
        engine.install(block.clone(), source).unwrap();
        let index = engine.record_index(key).unwrap();
        engine.records[index].as_mut().unwrap().compiled = Some(block);
        let mut runtime = TestRuntime {
            guard_valid: true,
            epoch: 0,
        };

        let execution = engine
            .execute_with_runtime(key, &mut frame(key), &mut runtime, None)
            .unwrap();
        assert_eq!(execution.tier, Mips4BlockTier::Interpreter);
        assert!(matches!(
            engine
                .execute_cached_with_runtime(key, &mut frame(key), &mut runtime, None, false,)
                .unwrap(),
            Mips4CachedBlockExecution::Executed(Mips4BlockExecution {
                tier: Mips4BlockTier::Interpreter,
                ..
            })
        ));
        assert_eq!(engine.backend.polls, 2);
    }

    #[test]
    fn stable_execution_records_only_generic_fast_fetches() {
        let source = Mips4BlockSource::Stable(code_guard());
        let block = block(0x1000, source);
        let key = block.key();
        let mut engine = Mips4BlockEngine::new(TestBackend);
        engine.install(block, source).unwrap();
        let mut runtime = TestRuntime {
            guard_valid: true,
            epoch: 0,
        };

        Mips4ExecutionPort::execute(&mut engine, key, &mut frame(key), &mut runtime, None).unwrap();

        assert_eq!(engine.statistics().fast_fetches, 1);
        assert_eq!(engine.statistics().dynamic_fetches, 0);
    }

    #[test]
    fn cache_capacity_reset_and_guard_invalidation_remain_engine_owned() {
        let mut engine = Mips4BlockEngine::new(TestBackend);
        for index in 0..=MIPS4_BLOCK_CACHE_CAPACITY {
            let block = block(0x1000 + index as u64 * 4, Mips4BlockSource::DynamicFetch);
            engine
                .install(block, Mips4BlockSource::DynamicFetch)
                .unwrap();
        }
        assert_eq!(engine.len(), 1);
        assert_eq!(engine.statistics().cache_resets, 1);

        let cached = block(0x8000, Mips4BlockSource::InstructionCache);
        let key = cached.key();
        engine
            .install(cached, Mips4BlockSource::InstructionCache)
            .unwrap();
        let runtime = TestRuntime {
            guard_valid: false,
            epoch: 1,
        };
        assert_eq!(
            engine.probe(key, Mips4BlockSource::InstructionCache, &runtime),
            Mips4BlockProbe::Missing
        );
        assert!(!engine.contains(key));
        assert_eq!(engine.statistics().invalidated_blocks, 1);
    }

    #[test]
    fn ordinary_and_reusable_entries_share_the_same_statistics_path() {
        let source = Mips4BlockSource::InstructionCache;
        let block = block(0x1000, source);
        let key = block.key();
        let mut ordinary = Mips4BlockEngine::new(TestBackend);
        let mut reusable = Mips4BlockEngine::new(TestBackend);
        ordinary.install(block.clone(), source).unwrap();
        reusable.install(block, source).unwrap();
        let mut ordinary_runtime = TestRuntime {
            guard_valid: true,
            epoch: 0,
        };
        let mut reusable_runtime = TestRuntime {
            guard_valid: true,
            epoch: 0,
        };
        Mips4ExecutionPort::execute(
            &mut ordinary,
            key,
            &mut frame(key),
            &mut ordinary_runtime,
            None,
        )
        .unwrap();
        assert!(matches!(
            Mips4ExecutionPort::execute_reusable(
                &mut reusable,
                key,
                &mut frame(key),
                &mut reusable_runtime,
                None,
                false,
            )
            .unwrap(),
            Mips4ReusableBlockExecution::Executed(_)
        ));
        assert_eq!(ordinary.statistics(), reusable.statistics());
    }

    #[test]
    fn reusable_batch_matches_scalar_entries_and_reports_missing_successor() {
        let source = Mips4BlockSource::InstructionCache;
        let blocks = [
            block(0x1000, source),
            block(0x1004, source),
            block(0x1008, source),
        ];
        let key = blocks[0].key();
        let mut batch_engine = Mips4BlockEngine::new(TestBackend);
        let mut scalar_engine = Mips4BlockEngine::new(TestBackend);
        for block in blocks {
            batch_engine.install(block.clone(), source).unwrap();
            scalar_engine.install(block, source).unwrap();
        }
        let mut batch_runtime = TestRuntime {
            guard_valid: true,
            epoch: 0,
        };
        let mut scalar_runtime = TestRuntime {
            guard_valid: true,
            epoch: 0,
        };
        let mut batch_frame = Mips4BlockFrame::new([0; 32], 0, 0, 0x1000, 0x1004, None, 4);
        let batch = Mips4ExecutionPort::execute_reusable_batch(
            &mut batch_engine,
            key,
            &mut batch_frame,
            &mut batch_runtime,
            None,
            false,
        )
        .unwrap();
        let Mips4ReusableBatchExecution::Executed(batch) = batch else {
            panic!("expected a completed cached batch");
        };
        assert_eq!(batch.stop, Mips4ReusableBatchStop::MissingSuccessor);
        assert_eq!(batch.entries, 3);
        assert_eq!(batch.execution.operations_executed, 3);

        let mut scalar_frame = Mips4BlockFrame::new([0; 32], 0, 0, 0x1000, 0x1004, None, 4);
        let mut scalar_key = key;
        for _ in 0..3 {
            assert!(matches!(
                Mips4ExecutionPort::execute_reusable(
                    &mut scalar_engine,
                    scalar_key,
                    &mut scalar_frame,
                    &mut scalar_runtime,
                    None,
                    false,
                )
                .unwrap(),
                Mips4ReusableBlockExecution::Executed(_)
            ));
            scalar_key.pc = scalar_frame.pc();
            scalar_key.next_pc = scalar_frame.next_pc();
        }
        assert_eq!(batch_frame.export_state(), scalar_frame.export_state());
        assert_eq!(batch_engine.statistics().interpreted_blocks, 3);
        assert_eq!(
            batch_engine.statistics().interpreted_blocks,
            scalar_engine.statistics().interpreted_blocks
        );
        assert_eq!(batch_engine.statistics().cached_dispatch_batches, 1);
        assert_eq!(batch_engine.statistics().cached_dispatch_entries, 3);
    }

    #[test]
    fn reusable_batch_stops_before_a_dirty_counter_barrier() {
        let source = Mips4BlockSource::InstructionCache;
        let first = block(0x1000, source);
        let second = block(0x1004, source);
        let first_key = first.key();
        let second_key = second.key();
        let mut engine = Mips4BlockEngine::new(TestBackend);
        engine.install(first, source).unwrap();
        engine.install(second, source).unwrap();
        let second_index = engine.record_index(second_key).unwrap();
        engine.records[second_index]
            .as_mut()
            .unwrap()
            .counter_barrier = true;
        let mut runtime = TestRuntime {
            guard_valid: true,
            epoch: 0,
        };
        let mut frame = Mips4BlockFrame::new([0; 32], 0, 0, 0x1000, 0x1004, None, 3);

        let batch = Mips4ExecutionPort::execute_reusable_batch(
            &mut engine,
            first_key,
            &mut frame,
            &mut runtime,
            None,
            false,
        )
        .unwrap();
        let Mips4ReusableBatchExecution::Executed(batch) = batch else {
            panic!("expected the prefix before the counter barrier");
        };
        assert_eq!(batch.stop, Mips4ReusableBatchStop::CounterSynchronization);
        assert_eq!(batch.entries, 1);
        assert_eq!(frame.pc(), second_key.pc);

        assert!(matches!(
            Mips4ExecutionPort::execute_reusable_batch(
                &mut engine,
                second_key,
                &mut frame,
                &mut runtime,
                None,
                true,
            )
            .unwrap(),
            Mips4ReusableBatchExecution::CounterSynchronization
        ));
        assert_eq!(engine.statistics().cached_dispatch_batches, 1);
        assert_eq!(engine.statistics().cached_dispatch_entries, 1);
    }

    #[test]
    fn region_construction_and_failed_retry_use_profiled_engine_state() {
        let source = Mips4BlockSource::Stable(code_guard());
        let block = block(0x1000, source);
        let key = block.key();
        let mut engine = Mips4BlockEngine::new(TestBackend);
        engine.insert(block).unwrap();
        let index = engine.record_index(key).unwrap();
        let record = engine.records[index].as_mut().unwrap();
        record.region_hotness = MIPS4_REGION_HOT_THRESHOLD;
        record.region_next_compile_hotness = MIPS4_REGION_HOT_THRESHOLD;
        record.successors[0] = Mips4SuccessorProfile {
            key: Some(key),
            observations: MIPS4_REGION_MIN_SUCCESSOR_OBSERVATIONS,
        };
        let region = build_profiled_region(index, &engine.indices, &engine.records).unwrap();
        assert_eq!(region.nodes().len(), 1);
        assert_eq!(region.nodes()[0].hot_successor(), Some(0));

        engine.records[index].as_mut().unwrap().successors = [Mips4SuccessorProfile::default(); 4];
        engine.maybe_compile_region(index).unwrap();
        let record = engine.records[index].as_ref().unwrap();
        assert!(record.compiled_region.is_none());
        assert_eq!(
            record.region_next_compile_hotness,
            MIPS4_REGION_HOT_THRESHOLD + MIPS4_REGION_RETRY_OPERATIONS
        );
    }

    #[test]
    fn region_construction_links_multiple_observed_direct_successors() {
        let source = Mips4BlockSource::Stable(code_guard());
        let first = block(0x1000, source);
        let second = block(0x1004, source);
        let third = block(0x1008, source);
        let first_key = first.key();
        let second_key = second.key();
        let third_key = third.key();
        let mut engine = Mips4BlockEngine::new(TestBackend);
        for block in [first, second, third] {
            engine.insert(block).unwrap();
        }
        let first_index = engine.record_index(first_key).unwrap();
        let second_index = engine.record_index(second_key).unwrap();
        let third_index = engine.record_index(third_key).unwrap();
        engine.records[first_index].as_mut().unwrap().region_hotness = MIPS4_REGION_HOT_THRESHOLD;
        engine.records[first_index].as_mut().unwrap().successors[0] = Mips4SuccessorProfile {
            key: Some(second_key),
            observations: 600,
        };
        engine.records[first_index].as_mut().unwrap().successors[1] = Mips4SuccessorProfile {
            key: Some(third_key),
            observations: 400,
        };
        engine.records[second_index].as_mut().unwrap().successors[0] = Mips4SuccessorProfile {
            key: Some(first_key),
            observations: MIPS4_REGION_MIN_SUCCESSOR_OBSERVATIONS,
        };
        engine.records[third_index].as_mut().unwrap().successors[0] = Mips4SuccessorProfile {
            key: Some(first_key),
            observations: MIPS4_REGION_MIN_SUCCESSOR_OBSERVATIONS,
        };

        let region = build_profiled_region(first_index, &engine.indices, &engine.records).unwrap();

        assert_eq!(region.nodes().len(), 3);
        assert_eq!(region.nodes()[0].successors(), &[1, 2]);
        assert_eq!(region.nodes()[1].successors(), &[0]);
        assert_eq!(region.nodes()[2].successors(), &[0]);
    }

    #[test]
    fn regions_reject_distinct_opaque_code_sources() {
        let first_guard = code_guard();
        let mut second_guard = Mips4CodeGuard {
            source_id: Mips4CodeSourceId::new(2),
            ..first_guard
        };
        let token_difference = first_guard.token() ^ second_guard.token();
        second_guard.fingerprint ^= token_difference.rotate_right(47);
        assert_eq!(first_guard.token(), second_guard.token());

        let first = block(0x1000, Mips4BlockSource::Stable(first_guard));
        let second = block(0x1004, Mips4BlockSource::Stable(second_guard));
        assert_eq!(first.key().code_guard, second.key().code_guard);
        let nodes = vec![
            Mips4RegionNode::new(first, Some(1)),
            Mips4RegionNode::new(second, Some(0)),
        ];

        assert!(Mips4Region::new(nodes).is_err());
    }
}
