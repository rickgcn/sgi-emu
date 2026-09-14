//! The interface for projecting a machine draft into a configuration view.

use crate::draft::MachineDraft;
use crate::id::MachineModelId;
use crate::view::ConfigurationView;

/// Describes a machine model and resolves user selections into a fresh view.
pub trait MachineDefinition: Send + Sync {
    /// Returns the stable identity of this model.
    fn model_id(&self) -> MachineModelId;

    /// Returns a user-facing name for this model.
    fn display_name(&self) -> &str;

    /// Returns an initial, editable configuration for this model.
    fn default_draft(&self) -> MachineDraft;

    /// Builds a new topology and diagnostics for the current draft.
    ///
    /// Invalid selections should remain visible and produce diagnostics rather
    /// than preventing the view from being returned.
    fn resolve(&self, draft: &MachineDraft) -> ConfigurationView;
}
