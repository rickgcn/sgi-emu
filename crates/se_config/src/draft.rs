//! User-editable configuration state.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::id::{DeviceKindId, MachineModelId, NodeId, PropertyId};
use crate::value::PropertyValue;

/// The user's current selections, including selections a definition may reject.
///
/// Topology is computed by a definition and is never stored in the draft.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MachineDraft {
    /// The model whose configuration is being edited.
    pub model: MachineModelId,
    /// Values selected for properties, ordered by stable property ID.
    pub properties: BTreeMap<PropertyId, PropertyValue>,
    /// Devices selected for slots, ordered by stable node ID.
    pub attachments: BTreeMap<NodeId, DeviceKindId>,
}

/// A single edit to the user's selections.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Edit {
    /// Set a property even when its ID or value is invalid for the definition.
    SetProperty {
        /// The property to edit.
        property: PropertyId,
        /// The requested value.
        value: PropertyValue,
    },
    /// Install, replace, or remove a device in a slot.
    SetAttachment {
        /// The slot to edit.
        slot: NodeId,
        /// The requested device, or `None` to empty the slot.
        device: Option<DeviceKindId>,
    },
}

impl MachineDraft {
    /// Applies an edit without validating or altering the requested value.
    pub fn apply(&mut self, edit: Edit) {
        match edit {
            Edit::SetProperty { property, value } => {
                self.properties.insert(property, value);
            }
            Edit::SetAttachment { slot, device } => {
                if let Some(device) = device {
                    self.attachments.insert(slot, device);
                } else {
                    self.attachments.remove(&slot);
                }
            }
        }
    }
}
