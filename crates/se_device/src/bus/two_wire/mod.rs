//! Open-drain two-wire communication domain.
//!
//! The bus combines participant outputs independently on the clock and data
//! lines. Protocol timing and message semantics belong to attached devices.

use core::fmt;
use std::collections::VecDeque;

use se_core::component::{Component, ComponentId};
use se_core::role::BusRole;
use se_core::scheduler::SimTime;

/// One participant's open-drain output transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct TwoWireDrive {
    /// Component changing its outputs.
    pub source: ComponentId,

    /// Simulation time at which the outputs change.
    pub time: SimTime,

    /// Whether the participant actively pulls the clock line low.
    pub clock_low: bool,

    /// Whether the participant actively pulls the data line low.
    pub data_low: bool,
}

/// One observed participant transition and resulting aggregate line levels.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct TwoWireLineDelivery {
    /// Bus on which the transition occurred.
    pub bus: ComponentId,

    /// Component whose outputs changed.
    pub source: ComponentId,

    /// Simulation time at which the outputs changed.
    pub time: SimTime,

    /// New open-drain clock state of the source.
    pub source_clock_low: bool,

    /// New open-drain data state of the source.
    pub source_data_low: bool,

    /// Whether at least one participant pulls the clock line low.
    pub clock_low: bool,

    /// Whether at least one participant pulls the data line low.
    pub data_low: bool,
}

/// Action emitted by a two-wire bus.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum TwoWireBusAction {
    /// Delivers one source transition to a participant.
    Deliver {
        /// Participant observing the lines.
        target: ComponentId,

        /// Source transition and aggregate line state.
        delivery: TwoWireLineDelivery,
    },

    /// No delivery is ready.
    Idle,
}

/// Two-wire bus construction error.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum TwoWireBusBuildError {
    /// The participant list contains the same component more than once.
    DuplicateParticipant(ComponentId),
}

impl fmt::Display for TwoWireBusBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateParticipant(component) => {
                write!(formatter, "duplicate two-wire participant {component}")
            }
        }
    }
}

impl std::error::Error for TwoWireBusBuildError {}

/// Two-wire bus routing error.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum TwoWireBusRouteError {
    /// A component not attached to the bus attempted to drive it.
    UnroutedSource(ComponentId),
}

impl fmt::Display for TwoWireBusRouteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnroutedSource(component) => {
                write!(formatter, "unrouted two-wire source {component}")
            }
        }
    }
}

impl std::error::Error for TwoWireBusRouteError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct ParticipantState {
    component: ComponentId,
    clock_low: bool,
    data_low: bool,
}

/// Combinational two-line open-drain bus.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct TwoWireBus {
    id: ComponentId,
    name: String,
    participants: Vec<ParticipantState>,
    actions: VecDeque<TwoWireBusAction>,
}

se_core::component_state!(TwoWireBusState, TwoWireBus);

impl TwoWireBus {
    /// Creates a bus with a fixed participant list.
    pub fn new(
        id: ComponentId,
        name: impl Into<String>,
        participants: impl IntoIterator<Item = ComponentId>,
    ) -> Result<Self, TwoWireBusBuildError> {
        let mut states = Vec::new();
        for component in participants {
            if states
                .iter()
                .any(|state: &ParticipantState| state.component == component)
            {
                return Err(TwoWireBusBuildError::DuplicateParticipant(component));
            }
            states.push(ParticipantState {
                component,
                clock_low: false,
                data_low: false,
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
    pub fn poll(&mut self) -> TwoWireBusAction {
        self.actions.pop_front().unwrap_or(TwoWireBusAction::Idle)
    }

    /// Returns whether the aggregate clock line is low.
    pub fn clock_low(&self) -> bool {
        self.participants.iter().any(|state| state.clock_low)
    }

    /// Returns whether the aggregate data line is low.
    pub fn data_low(&self) -> bool {
        self.participants.iter().any(|state| state.data_low)
    }

    fn reset_state(&mut self) {
        for state in &mut self.participants {
            state.clock_low = false;
            state.data_low = false;
        }
        self.actions.clear();
    }
}

impl Component for TwoWireBus {
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

impl BusRole<TwoWireDrive> for TwoWireBus {
    type Response = Result<(), TwoWireBusRouteError>;

    fn route(&mut self, drive: TwoWireDrive) -> Self::Response {
        let Some(state) = self
            .participants
            .iter_mut()
            .find(|state| state.component == drive.source)
        else {
            return Err(TwoWireBusRouteError::UnroutedSource(drive.source));
        };
        if state.clock_low == drive.clock_low && state.data_low == drive.data_low {
            return Ok(());
        }
        state.clock_low = drive.clock_low;
        state.data_low = drive.data_low;
        let delivery = TwoWireLineDelivery {
            bus: self.id,
            source: drive.source,
            time: drive.time,
            source_clock_low: drive.clock_low,
            source_data_low: drive.data_low,
            clock_low: self.clock_low(),
            data_low: self.data_low(),
        };
        for state in &self.participants {
            self.actions.push_back(TwoWireBusAction::Deliver {
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
    fn lines_are_combined_and_delivered_to_every_participant() {
        let mut bus = TwoWireBus::new(component(1), "DDC", [component(2), component(3)])
            .expect("unique participants must build");
        bus.route(TwoWireDrive {
            source: component(2),
            time: SimTime::new(10),
            clock_low: true,
            data_low: false,
        })
        .unwrap();
        for target in [component(2), component(3)] {
            assert_eq!(
                bus.poll(),
                TwoWireBusAction::Deliver {
                    target,
                    delivery: TwoWireLineDelivery {
                        bus: component(1),
                        source: component(2),
                        time: SimTime::new(10),
                        source_clock_low: true,
                        source_data_low: false,
                        clock_low: true,
                        data_low: false,
                    },
                }
            );
        }
    }

    #[test]
    fn one_participant_cannot_release_another_participants_low_line() {
        let mut bus = TwoWireBus::new(component(1), "DDC", [component(2), component(3)]).unwrap();
        for drive in [
            TwoWireDrive {
                source: component(2),
                time: SimTime::new(1),
                clock_low: false,
                data_low: true,
            },
            TwoWireDrive {
                source: component(3),
                time: SimTime::new(2),
                clock_low: true,
                data_low: false,
            },
            TwoWireDrive {
                source: component(2),
                time: SimTime::new(3),
                clock_low: false,
                data_low: false,
            },
        ] {
            bus.route(drive).unwrap();
            for _ in 0..2 {
                let TwoWireBusAction::Deliver { delivery, .. } = bus.poll() else {
                    panic!("line transition must be delivered");
                };
                if drive.time == SimTime::new(3) {
                    assert!(delivery.clock_low);
                    assert!(!delivery.data_low);
                }
            }
        }
    }

    #[test]
    fn reset_releases_both_lines_and_unknown_sources_are_rejected() {
        let mut bus = TwoWireBus::new(component(1), "DDC", [component(2)]).unwrap();
        bus.route(TwoWireDrive {
            source: component(2),
            time: SimTime::ZERO,
            clock_low: true,
            data_low: true,
        })
        .unwrap();
        bus.reset();
        assert!(!bus.clock_low());
        assert!(!bus.data_low());
        assert_eq!(
            bus.route(TwoWireDrive {
                source: component(3),
                time: SimTime::ZERO,
                clock_low: false,
                data_low: false,
            }),
            Err(TwoWireBusRouteError::UnroutedSource(component(3)))
        );
    }

    #[test]
    fn serialized_state_preserves_participant_drives_and_pending_deliveries() {
        let mut reference =
            TwoWireBus::new(component(1), "DDC", [component(2), component(3)]).unwrap();
        reference
            .route(TwoWireDrive {
                source: component(2),
                time: SimTime::new(9),
                clock_low: true,
                data_low: true,
            })
            .unwrap();
        let encoded = postcard::to_stdvec(&reference.save_state()).unwrap();
        let state: TwoWireBusState = postcard::from_bytes(&encoded).unwrap();
        let mut restored =
            TwoWireBus::new(component(1), "DDC", [component(2), component(3)]).unwrap();
        restored.restore_state(state).unwrap();
        assert!(restored.clock_low());
        assert!(restored.data_low());
        assert_eq!(restored.poll(), reference.poll());
        assert_eq!(restored.poll(), reference.poll());
    }
}
