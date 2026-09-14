//! Selection and startup policy for the currently supported machine.

use std::sync::Arc;

use se_config::definition::MachineDefinition;
use se_config::diagnostic::DiagnosticSeverity;
use se_config::draft::MachineDraft;
use se_config::id::PropertyId;
use se_config::value::PropertyValue;
use se_machine::indigo::ip12::definition::Ip12Definition;

/// Returns the definition used by the machine settings view.
#[must_use]
pub fn machine_definition() -> Arc<dyn MachineDefinition> {
    Arc::new(Ip12Definition)
}

/// Returns the initial machine configuration.
#[must_use]
pub fn default_machine_draft() -> MachineDraft {
    Ip12Definition.default_draft()
}

/// Whether startup should attempt cold machine construction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartupReadiness {
    /// The default firmware path has not yet been selected.
    Unconfigured,
    /// Construction should run and report any semantic or host error.
    Ready,
}

/// Applies the startup-only empty firmware exception.
#[must_use]
pub fn startup_readiness(draft: &MachineDraft) -> StartupReadiness {
    let firmware_unselected = matches!(
        draft
        .properties
        .get(&PropertyId(String::from("firmware.0.image-path"))),
        Some(PropertyValue::Text(path)) if path.is_empty()
    );
    if firmware_unselected
        && Ip12Definition
            .resolve(draft)
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
            .all(|diagnostic| diagnostic.code == "ip12.firmware.image-required")
    {
        StartupReadiness::Unconfigured
    } else {
        StartupReadiness::Ready
    }
}

#[cfg(test)]
mod tests {
    use se_config::draft::Edit;
    use se_config::id::{MachineModelId, PropertyId};
    use se_config::value::PropertyValue;

    use super::{StartupReadiness, default_machine_draft, startup_readiness};

    #[test]
    fn only_the_unselected_firmware_exception_skips_startup() {
        let mut draft = default_machine_draft();
        assert_eq!(startup_readiness(&draft), StartupReadiness::Unconfigured);
        draft
            .properties
            .remove(&PropertyId(String::from("firmware.0.image-path")));
        assert_eq!(startup_readiness(&draft), StartupReadiness::Ready);
        draft = default_machine_draft();
        draft.apply(Edit::SetProperty {
            property: PropertyId(String::from("memory.bank.a.simm-mib")),
            value: PropertyValue::Integer(0),
        });
        assert_eq!(startup_readiness(&draft), StartupReadiness::Ready);
        draft = default_machine_draft();
        draft.model = MachineModelId(String::from("other-model"));
        assert_eq!(startup_readiness(&draft), StartupReadiness::Ready);
    }
}
