//! Level-sensitive interrupt routing bus.

use core::fmt;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use se_core::component::{Component, ComponentId};
use se_core::role::BusRole;

/// Device-local interrupt output identifier.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub struct IrqOutput(u16);

impl IrqOutput {
    /// Creates an interrupt output identifier.
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    /// Returns the underlying device-local identifier.
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// Device-local interrupt input identifier.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub struct IrqInput(u16);

impl IrqInput {
    /// Creates an interrupt input identifier.
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    /// Returns the underlying device-local identifier.
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// Interrupt source endpoint.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub struct IrqSource {
    /// Source component.
    pub component: ComponentId,

    /// Source-local output.
    pub output: IrqOutput,
}

/// Interrupt target endpoint.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub struct IrqTarget {
    /// Target component.
    pub component: ComponentId,

    /// Target-local input.
    pub input: IrqInput,
}

/// Configured interrupt connection.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub struct IrqRoute {
    /// Source endpoint.
    pub source: IrqSource,

    /// Target endpoint.
    pub target: IrqTarget,
}

/// Source-driven interrupt level update.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct IrqTransaction {
    /// Source endpoint changing level.
    pub source: IrqSource,

    /// New source level.
    pub asserted: bool,
}

/// Aggregated interrupt level delivered to a target.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct IrqDelivery {
    /// Target-local input.
    pub input: IrqInput,

    /// New wired-OR input level.
    pub asserted: bool,
}

/// Action emitted by an interrupt bus.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum IrqBusAction {
    /// Delivers one changed input level.
    Deliver {
        /// Target component.
        target: ComponentId,

        /// Target-local delivery.
        delivery: IrqDelivery,
    },

    /// No action is ready.
    Idle,
}

/// Interrupt bus construction error.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum IrqBusBuildError {
    /// The routing table contains the same connection more than once.
    DuplicateRoute(IrqRoute),
}

impl fmt::Display for IrqBusBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateRoute(route) => write!(
                f,
                "duplicate IRQ route from {} output {} to {} input {}",
                route.source.component,
                route.source.output.get(),
                route.target.component,
                route.target.input.get()
            ),
        }
    }
}

impl std::error::Error for IrqBusBuildError {}

/// Interrupt transaction routing error.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum IrqBusRouteError {
    /// No configured route begins at the transaction source.
    UnroutedSource(IrqSource),
}

impl fmt::Display for IrqBusRouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnroutedSource(source) => write!(
                f,
                "unrouted IRQ source {} output {}",
                source.component,
                source.output.get()
            ),
        }
    }
}

impl std::error::Error for IrqBusRouteError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct RouteState {
    route: IrqRoute,
    asserted: bool,
}

/// Combinational level-sensitive interrupt routing domain.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct IrqBus {
    id: ComponentId,
    name: String,
    routes: Vec<RouteState>,
    target_levels: BTreeMap<IrqTarget, bool>,
    actions: VecDeque<IrqBusAction>,
}

crate::component_state!(IrqBusState, IrqBus);

impl IrqBus {
    /// Creates an interrupt bus with a fixed routing table.
    pub fn new(
        id: ComponentId,
        name: impl Into<String>,
        routes: impl IntoIterator<Item = IrqRoute>,
    ) -> Result<Self, IrqBusBuildError> {
        let mut seen = BTreeSet::new();
        let mut route_states = Vec::new();
        let mut target_levels = BTreeMap::new();
        for route in routes {
            if !seen.insert(route) {
                return Err(IrqBusBuildError::DuplicateRoute(route));
            }
            target_levels.entry(route.target).or_insert(false);
            route_states.push(RouteState {
                route,
                asserted: false,
            });
        }
        Ok(Self {
            id,
            name: name.into(),
            routes: route_states,
            target_levels,
            actions: VecDeque::new(),
        })
    }

    /// Polls one pending delivery.
    pub fn poll(&mut self) -> IrqBusAction {
        self.actions.pop_front().unwrap_or(IrqBusAction::Idle)
    }

    fn reset_state(&mut self) {
        for route in &mut self.routes {
            route.asserted = false;
        }
        for asserted in self.target_levels.values_mut() {
            *asserted = false;
        }
        self.actions.clear();
    }
}

impl Component for IrqBus {
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

impl BusRole<IrqTransaction> for IrqBus {
    type Response = Result<(), IrqBusRouteError>;

    fn route(&mut self, transaction: IrqTransaction) -> Self::Response {
        let mut targets = Vec::new();
        for state in &mut self.routes {
            if state.route.source != transaction.source {
                continue;
            }
            state.asserted = transaction.asserted;
            if !targets.contains(&state.route.target) {
                targets.push(state.route.target);
            }
        }
        if targets.is_empty() {
            return Err(IrqBusRouteError::UnroutedSource(transaction.source));
        }
        for target in targets {
            let asserted = self
                .routes
                .iter()
                .any(|state| state.route.target == target && state.asserted);
            let previous = self
                .target_levels
                .insert(target, asserted)
                .expect("every configured IRQ target must have level state");
            if previous != asserted {
                self.actions.push_back(IrqBusAction::Deliver {
                    target: target.component,
                    delivery: IrqDelivery {
                        input: target.input,
                        asserted,
                    },
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
