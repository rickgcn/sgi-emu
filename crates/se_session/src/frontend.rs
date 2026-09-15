//! Frontend peripherals projected from one validated machine build plan.

use se_machine::endpoint::EndpointKey;
use se_machine::indigo::ip12::plan::{Ip12BuildPlan, Ip12Peripheral, Ip12Port};
use se_runtime::runtime::RuntimeConfiguration;

/// Frontend-provided peripherals attached to the active machine.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FrontendPlan {
    serial_consoles: Vec<EndpointKey>,
}

impl FrontendPlan {
    pub(crate) fn from_ip12(plan: &Ip12BuildPlan) -> Self {
        let mut serial_consoles = Vec::new();
        for attachment in plan.port_attachments() {
            match attachment.peripheral() {
                Ip12Peripheral::SgiKeyboard | Ip12Peripheral::SgiMouse => {}
                Ip12Peripheral::Vt100Terminal => {
                    assert!(matches!(
                        attachment.port(),
                        Ip12Port::SerialA | Ip12Port::SerialB
                    ));
                    let endpoint = attachment.port().endpoint_key();
                    assert!(
                        !serial_consoles.contains(&endpoint),
                        "an IP12 frontend plan cannot contain a duplicate serial console"
                    );
                    serial_consoles.push(endpoint);
                }
            }
        }
        Self { serial_consoles }
    }

    /// Returns serial interfaces with an attached frontend VT100 terminal.
    #[must_use]
    pub fn serial_console_endpoints(&self) -> &[EndpointKey] {
        &self.serial_consoles
    }
}

/// A runtime configuration and its matching frontend peripheral projection.
pub struct SessionBuild {
    runtime: RuntimeConfiguration,
    frontend: FrontendPlan,
}

impl SessionBuild {
    pub(crate) const fn new(runtime: RuntimeConfiguration, frontend: FrontendPlan) -> Self {
        Self { runtime, frontend }
    }

    /// Separates the runtime configuration and matching frontend projection.
    #[must_use]
    pub fn into_parts(self) -> (RuntimeConfiguration, FrontendPlan) {
        (self.runtime, self.frontend)
    }
}

#[cfg(test)]
mod tests {
    use se_config::definition::MachineDefinition;
    use se_config::draft::Edit;
    use se_config::id::{NodeId, PropertyId};
    use se_config::value::PropertyValue;
    use se_machine::indigo::ip12::definition::Ip12Definition;

    use super::FrontendPlan;

    fn plan_with(serial_a: bool, serial_b: bool) -> se_machine::indigo::ip12::plan::Ip12BuildPlan {
        let mut draft = Ip12Definition.default_draft();
        draft.apply(Edit::SetProperty {
            property: PropertyId(String::from("firmware.0.image-path")),
            value: PropertyValue::Text(String::from("unused.prom")),
        });
        for (port, attached) in [
            ("serial.1.channel.a.port", serial_a),
            ("serial.1.channel.b.port", serial_b),
        ] {
            if !attached {
                draft.apply(Edit::SetAttachment {
                    slot: NodeId(String::from(port)),
                    device: None,
                });
            }
        }
        Ip12Definition.compile(&draft).expect("the draft is valid")
    }

    #[test]
    fn frontend_plan_preserves_canonical_serial_console_order() {
        let plan = FrontendPlan::from_ip12(&plan_with(true, true));
        assert_eq!(
            plan.serial_console_endpoints()
                .iter()
                .map(|endpoint| endpoint.as_str())
                .collect::<Vec<_>>(),
            ["serial.external.a", "serial.external.b"]
        );
    }

    #[test]
    fn frontend_plan_projects_each_partial_serial_configuration() {
        for (serial_a, serial_b, expected) in [
            (false, false, Vec::<&str>::new()),
            (true, false, vec!["serial.external.a"]),
            (false, true, vec!["serial.external.b"]),
        ] {
            let plan = FrontendPlan::from_ip12(&plan_with(serial_a, serial_b));
            assert_eq!(
                plan.serial_console_endpoints()
                    .iter()
                    .map(|endpoint| endpoint.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }
}
