//! Stable, opaque identifiers used by configuration definitions and drafts.

use serde::{Deserialize, Serialize};

/// Identifies a machine model.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MachineModelId(pub String);

/// Identifies a node without encoding its position in the topology.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub String);

/// Identifies an editable property.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PropertyId(pub String);

/// Identifies a kind of device that can be selected for a slot.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceKindId(pub String);
