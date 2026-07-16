//! Open-drain 1-Wire communication domain.
//!
//! The bus combines participant outputs as a wired-AND line. Protocol timing
//! and command semantics belong to the attached controller and devices.

use core::fmt;
use std::collections::VecDeque;

use se_core::component::{Component, ComponentId};
use se_core::role::BusRole;
use se_core::scheduler::SimTime;

/// One participant's open-drain output transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct OneWireDrive {
    /// Component changing its output.
    pub source: ComponentId,

    /// Simulation time at which the output changes.
    pub time: SimTime,

    /// Whether the participant actively pulls the line low.
    pub drive_low: bool,
}

/// One observed participant transition and resulting aggregate line level.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct OneWireLineDelivery {
    /// Component whose output changed.
    pub source: ComponentId,

    /// Simulation time at which the output changed.
    pub time: SimTime,

    /// New open-drain state of the source.
    pub source_drive_low: bool,

    /// Whether at least one participant now pulls the line low.
    pub line_low: bool,
}

/// Action emitted by a 1-Wire bus.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum OneWireBusAction {
    /// Delivers one source transition to a participant.
    Deliver {
        /// Participant observing the line.
        target: ComponentId,

        /// Source transition and aggregate line state.
        delivery: OneWireLineDelivery,
    },

    /// No delivery is ready.
    Idle,
}

/// 1-Wire bus construction error.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum OneWireBusBuildError {
    /// The participant list contains the same component more than once.
    DuplicateParticipant(ComponentId),
}

impl fmt::Display for OneWireBusBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateParticipant(component) => {
                write!(formatter, "duplicate 1-Wire participant {component}")
            }
        }
    }
}

impl std::error::Error for OneWireBusBuildError {}

/// 1-Wire routing error.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum OneWireBusRouteError {
    /// A component not attached to the bus attempted to drive it.
    UnroutedSource(ComponentId),
}

impl fmt::Display for OneWireBusRouteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnroutedSource(component) => {
                write!(formatter, "unrouted 1-Wire source {component}")
            }
        }
    }
}

impl std::error::Error for OneWireBusRouteError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct ParticipantState {
    component: ComponentId,
    drive_low: bool,
}

/// Combinational open-drain 1-Wire bus.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct OneWireBus {
    id: ComponentId,
    name: String,
    participants: Vec<ParticipantState>,
    actions: VecDeque<OneWireBusAction>,
}

se_core::component_state!(OneWireBusState, OneWireBus);

impl OneWireBus {
    /// Creates a bus with a fixed participant list.
    pub fn new(
        id: ComponentId,
        name: impl Into<String>,
        participants: impl IntoIterator<Item = ComponentId>,
    ) -> Result<Self, OneWireBusBuildError> {
        let mut states = Vec::new();
        for component in participants {
            if states
                .iter()
                .any(|state: &ParticipantState| state.component == component)
            {
                return Err(OneWireBusBuildError::DuplicateParticipant(component));
            }
            states.push(ParticipantState {
                component,
                drive_low: false,
            });
        }
        Ok(Self {
            id,
            name: name.into(),
            participants: states,
            actions: VecDeque::new(),
        })
    }

    /// Polls one pending line observation.
    pub fn poll(&mut self) -> OneWireBusAction {
        self.actions.pop_front().unwrap_or(OneWireBusAction::Idle)
    }

    /// Returns whether the aggregate line is low.
    pub fn line_low(&self) -> bool {
        self.participants.iter().any(|state| state.drive_low)
    }

    fn reset_state(&mut self) {
        for state in &mut self.participants {
            state.drive_low = false;
        }
        self.actions.clear();
    }
}

impl Component for OneWireBus {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn reset(&mut self) {
        self.reset_state();
    }
}

impl BusRole<OneWireDrive> for OneWireBus {
    type Response = Result<(), OneWireBusRouteError>;

    fn route(&mut self, drive: OneWireDrive) -> Self::Response {
        let Some(state) = self
            .participants
            .iter_mut()
            .find(|state| state.component == drive.source)
        else {
            return Err(OneWireBusRouteError::UnroutedSource(drive.source));
        };
        if state.drive_low == drive.drive_low {
            return Ok(());
        }
        state.drive_low = drive.drive_low;
        let line_low = self.line_low();
        let delivery = OneWireLineDelivery {
            source: drive.source,
            time: drive.time,
            source_drive_low: drive.drive_low,
            line_low,
        };
        for state in &self.participants {
            self.actions.push_back(OneWireBusAction::Deliver {
                target: state.component,
                delivery,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn component(value: u64) -> ComponentId {
        ComponentId::new(value)
    }

    #[test]
    fn open_drain_levels_are_delivered_to_every_participant() {
        let mut bus = OneWireBus::new(component(1), "1-Wire", [component(2), component(3)])
            .expect("unique participants must build");
        bus.route(OneWireDrive {
            source: component(2),
            time: SimTime::new(10),
            drive_low: true,
        })
        .unwrap();

        for target in [component(2), component(3)] {
            assert_eq!(
                bus.poll(),
                OneWireBusAction::Deliver {
                    target,
                    delivery: OneWireLineDelivery {
                        source: component(2),
                        time: SimTime::new(10),
                        source_drive_low: true,
                        line_low: true,
                    },
                }
            );
        }
        assert_eq!(bus.poll(), OneWireBusAction::Idle);
    }

    #[test]
    fn every_source_transition_is_visible_while_another_source_holds_low() {
        let mut bus = OneWireBus::new(component(1), "1-Wire", [component(2), component(3)])
            .expect("unique participants must build");
        for (source, drive_low) in [
            (component(2), true),
            (component(3), true),
            (component(2), false),
        ] {
            bus.route(OneWireDrive {
                source,
                time: SimTime::new(20),
                drive_low,
            })
            .unwrap();
            for _ in 0..2 {
                let OneWireBusAction::Deliver { delivery, .. } = bus.poll() else {
                    panic!("source transition must be delivered");
                };
                assert!(delivery.line_low);
                assert_eq!(delivery.source, source);
            }
        }
    }

    #[test]
    fn repeated_levels_are_suppressed_and_reset_releases_the_line() {
        let mut bus = OneWireBus::new(component(1), "1-Wire", [component(2)])
            .expect("unique participants must build");
        let drive = OneWireDrive {
            source: component(2),
            time: SimTime::new(1),
            drive_low: true,
        };
        bus.route(drive).unwrap();
        assert!(matches!(bus.poll(), OneWireBusAction::Deliver { .. }));
        bus.route(drive).unwrap();
        assert_eq!(bus.poll(), OneWireBusAction::Idle);
        bus.reset();
        assert!(!bus.line_low());
    }

    #[test]
    fn construction_and_routing_reject_unknown_endpoints() {
        assert_eq!(
            OneWireBus::new(component(1), "1-Wire", [component(2), component(2)]),
            Err(OneWireBusBuildError::DuplicateParticipant(component(2)))
        );
        let mut bus = OneWireBus::new(component(1), "1-Wire", [component(2)]).unwrap();
        assert_eq!(
            bus.route(OneWireDrive {
                source: component(3),
                time: SimTime::ZERO,
                drive_low: false,
            }),
            Err(OneWireBusRouteError::UnroutedSource(component(3)))
        );
    }
}
