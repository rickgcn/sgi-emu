//! Bounded MIPS IV control-flow Regions owned by the host execution engine.

use core::fmt;
use se_device::cpu::mips4::cp1::decode::{Mips4Cp1Decode, Mips4Cp1InstructionClass};

use se_device::cpu::mips4::execution::block::{
    Mips4Block, Mips4BlockBuildError, Mips4BlockGuard, Mips4BlockKey, Mips4RuntimeOperation,
};

use super::engine::{
    MIPS4_REGION_MAX_NODES, MIPS4_REGION_MAX_OPERATIONS, MIPS4_REGION_MIN_ACYCLIC_OPERATIONS,
    Mips4RegionSource, region_source_kind,
};

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

    pub(super) fn set_hot_successor(&mut self, hot_successor: Option<usize>) {
        self.hot_successor = hot_successor;
    }
}

/// Verified bounded control-flow Region compiled as one native function.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mips4Region {
    key: Mips4RegionKey,
    nodes: Vec<Mips4RegionNode>,
}

/// Failure to construct or verify a host-owned MIPS IV Region.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Mips4RegionBuildError {
    /// A member block failed portable structural verification.
    InvalidBlock(Mips4BlockBuildError),
    /// Region sources, bounds, or control-flow topology were inconsistent.
    InvalidTopology,
}

impl fmt::Display for Mips4RegionBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBlock(error) => error.fmt(formatter),
            Self::InvalidTopology => {
                formatter.write_str("MIPS IV Region has an invalid execution topology")
            }
        }
    }
}

impl std::error::Error for Mips4RegionBuildError {}

impl Mips4Region {
    /// Creates and verifies a bounded Region from unique block nodes.
    pub fn new(nodes: Vec<Mips4RegionNode>) -> Result<Self, Mips4RegionBuildError> {
        let entry = nodes
            .first()
            .ok_or(Mips4RegionBuildError::InvalidTopology)?
            .block
            .key();
        let region = Self {
            key: Mips4RegionKey { entry },
            nodes,
        };
        region.verify()?;
        Ok(region)
    }

    pub(super) fn runtime_operations(&self) -> Vec<Mips4RuntimeOperation> {
        self.nodes
            .iter()
            .flat_map(|node| node.block.runtime_operations())
            .collect()
    }

    pub(super) fn member_keys(&self) -> Vec<Mips4BlockKey> {
        self.nodes.iter().map(|node| node.block.key()).collect()
    }

    pub(super) fn guards(&self) -> Vec<Mips4BlockGuard> {
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

    fn build_error(&self) -> Result<(), Mips4RegionBuildError> {
        if self.is_executable() {
            Ok(())
        } else {
            Err(Mips4RegionBuildError::InvalidTopology)
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
    pub fn verify(&self) -> Result<(), Mips4RegionBuildError> {
        if self.nodes.is_empty() || self.nodes.len() > MIPS4_REGION_MAX_NODES {
            return Err(Mips4RegionBuildError::InvalidTopology);
        }
        let mut operations = 0_usize;
        for node in &self.nodes {
            node.block
                .verify()
                .map_err(Mips4RegionBuildError::InvalidBlock)?;
            operations = operations.saturating_add(node.block.instruction_count());
            if operations > MIPS4_REGION_MAX_OPERATIONS
                || node
                    .hot_successor
                    .is_some_and(|successor| successor >= self.nodes.len())
            {
                return Err(Mips4RegionBuildError::InvalidTopology);
            }
            let key = node.block.key();
            if key.fetch_context != self.key.entry.fetch_context
                || key.translation_generation != self.key.entry.translation_generation
                || key.code_guard != self.key.entry.code_guard
            {
                return Err(Mips4RegionBuildError::InvalidTopology);
            }
        }
        if self.nodes[0].block.key() != self.key.entry {
            return Err(Mips4RegionBuildError::InvalidTopology);
        }
        let source_kind = self
            .source_kind()
            .ok_or(Mips4RegionBuildError::InvalidTopology)?;
        if self
            .nodes
            .iter()
            .any(|node| region_source_kind(node.block.guard()) != Some(source_kind))
        {
            return Err(Mips4RegionBuildError::InvalidTopology);
        }
        self.build_error()
    }
}

/// Reason a native Region returned to the Rust dispatcher.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4RegionSideExit {
    /// The selected successor is outside the Region.
    ColdSuccessor,
    /// The remaining budget cannot admit another node.
    Budget,
    /// A typed runtime helper left native execution.
    Runtime,
    /// A visibility guard or execution epoch changed.
    Guard,
}
