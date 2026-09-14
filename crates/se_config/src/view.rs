//! A resolved, read-only snapshot of the configuration topology.

use crate::diagnostic::Diagnostic;
use crate::id::{DeviceKindId, MachineModelId, NodeId, PropertyId};
use crate::value::PropertyValue;

/// A machine definition's projection of one draft.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigurationView {
    /// The model represented by this view.
    pub model: MachineModelId,
    /// The user-facing name of the model.
    pub display_name: String,
    /// Nodes in parent-before-child order.
    pub nodes: Vec<TopologyNode>,
    /// Problems found in the draft while constructing the view.
    pub diagnostics: Vec<Diagnostic>,
}

/// A node with an explicit parent in a resolved topology.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TopologyNode {
    /// The stable identity of this node.
    pub id: NodeId,
    /// The parent node, or `None` for a root.
    pub parent: Option<NodeId>,
    /// The structural role of this node.
    pub role: NodeRole,
    /// The user-facing label of this node.
    pub label: String,
    /// Properties editable on this node.
    pub properties: Vec<PropertyView>,
    /// An attachment editor when this node is a configurable slot.
    pub attachment: Option<AttachmentView>,
}

/// A structural role without hardware-specific meaning.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeRole {
    /// The root of a configuration tree.
    Root,
    /// A component that may contain other nodes.
    Component,
    /// A location that may accept a device.
    Slot,
    /// A device selected for a slot.
    Device,
    /// An externally visible endpoint.
    Endpoint,
}

/// A resolved property value and its editor metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PropertyView {
    /// The stable identity of the property.
    pub id: PropertyId,
    /// The user-facing label of the property.
    pub label: String,
    /// The current value, including an invalid value supplied by the draft.
    pub value: PropertyValue,
    /// The editor to present for this property.
    pub editor: PropertyEditor,
}

/// Metadata describing how a primitive property value can be edited.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PropertyEditor {
    /// A Boolean toggle.
    Toggle,
    /// A bounded integer editor.
    Integer {
        /// The smallest supported value.
        min: i64,
        /// The largest supported value.
        max: i64,
        /// The step used by the editor.
        step: i64,
        /// An optional display unit.
        unit: Option<String>,
    },
    /// A free-form text editor.
    Text,
    /// A path editor backed by a text value.
    Path {
        /// The kind of path requested by the editor.
        kind: PathKind,
    },
    /// A finite set of primitive values.
    Choice {
        /// The choices offered by the editor.
        options: Vec<ChoiceOption>,
    },
}

/// The kind of path selected by a path editor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathKind {
    /// A file path.
    File,
    /// A directory path.
    Directory,
}

/// One labeled value offered by a choice editor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChoiceOption {
    /// The value stored in the draft when selected.
    pub value: PropertyValue,
    /// The user-facing label of the choice.
    pub label: String,
}

/// The current selection and available devices for a slot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttachmentView {
    /// Whether the slot may be empty.
    pub allow_empty: bool,
    /// The selected device kind, including an unsupported selection.
    pub current: Option<DeviceKindId>,
    /// Device kinds offered by this slot.
    pub choices: Vec<DeviceChoice>,
}

/// A labeled device kind offered by an attachment editor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceChoice {
    /// The stable identity of the device kind.
    pub id: DeviceKindId,
    /// The user-facing label of the device kind.
    pub label: String,
}
