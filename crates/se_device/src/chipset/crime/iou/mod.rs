//! Functional CRIME peer-link correlation state.

use std::collections::BTreeMap;

use se_core::component::{
    Component, ComponentId, ComponentStateError, validate_component_state_id,
};

use super::protocol::{
    CrimeCgiCompletion, CrimeCgiTransaction, CrimeCmiCompletion, CrimeCmiTransaction,
    CrimeTransactionId,
};

type LinkKey = (ComponentId, CrimeTransactionId);

/// CRIME-to-MACE CMI link state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrimeCmiBus {
    id: ComponentId,
    name: String,
    in_flight: BTreeMap<LinkKey, CrimeCmiTransaction>,
}

/// Serializable CMI transaction-correlation state.
#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct CrimeCmiBusState {
    id: ComponentId,
    in_flight: BTreeMap<LinkKey, CrimeCmiTransaction>,
}

impl CrimeCmiBus {
    /// Creates an empty CMI link.
    pub fn new(id: ComponentId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            in_flight: BTreeMap::new(),
        }
    }

    /// Captures outstanding CMI requests.
    pub fn save_state(&self) -> CrimeCmiBusState {
        CrimeCmiBusState {
            id: self.id,
            in_flight: self.in_flight.clone(),
        }
    }

    /// Restores validated CMI correlation state.
    pub fn restore_state(&mut self, state: CrimeCmiBusState) -> Result<(), ComponentStateError> {
        validate_component_state_id(self.id, state.id)?;
        if !state
            .in_flight
            .iter()
            .all(|(key, transaction)| *key == (transaction.target, transaction.id))
        {
            return Err(ComponentStateError::InvalidState {
                component: self.id,
                invariant: "CMI request keys must match their target and transaction identifier",
            });
        }
        self.in_flight = state.in_flight;
        Ok(())
    }

    /// Records one request before the runtime delivers it to its target.
    #[must_use]
    pub fn begin(&mut self, transaction: &CrimeCmiTransaction) -> bool {
        self.in_flight
            .insert((transaction.target, transaction.id), transaction.clone())
            .is_none()
    }

    /// Correlates one target completion with its controller.
    pub fn complete(
        &mut self,
        target: ComponentId,
        completion: CrimeCmiCompletion,
    ) -> Option<(ComponentId, CrimeCmiCompletion)> {
        let transaction = self.in_flight.remove(&(target, completion.id))?;
        Some((transaction.controller, completion))
    }

    /// Cancels all outstanding CMI requests.
    pub fn hard_reset(&mut self) {
        self.in_flight.clear();
    }

    /// Returns the number of outstanding requests.
    pub fn pending_transactions(&self) -> usize {
        self.in_flight.len()
    }
}

impl Component for CrimeCmiBus {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn reset(&mut self) {
        self.hard_reset();
    }
}

/// CRIME-to-GBE CGI link state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrimeCgiBus {
    id: ComponentId,
    name: String,
    in_flight: BTreeMap<LinkKey, CrimeCgiTransaction>,
}

/// Serializable CGI transaction-correlation state.
#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct CrimeCgiBusState {
    id: ComponentId,
    in_flight: BTreeMap<LinkKey, CrimeCgiTransaction>,
}

impl CrimeCgiBus {
    /// Creates an empty CGI link.
    pub fn new(id: ComponentId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            in_flight: BTreeMap::new(),
        }
    }

    /// Captures outstanding CGI requests.
    pub fn save_state(&self) -> CrimeCgiBusState {
        CrimeCgiBusState {
            id: self.id,
            in_flight: self.in_flight.clone(),
        }
    }

    /// Restores validated CGI correlation state.
    pub fn restore_state(&mut self, state: CrimeCgiBusState) -> Result<(), ComponentStateError> {
        validate_component_state_id(self.id, state.id)?;
        if !state
            .in_flight
            .iter()
            .all(|(key, transaction)| *key == (transaction.target, transaction.id))
        {
            return Err(ComponentStateError::InvalidState {
                component: self.id,
                invariant: "CGI request keys must match their target and transaction identifier",
            });
        }
        self.in_flight = state.in_flight;
        Ok(())
    }

    /// Records one request before the runtime delivers it to its target.
    #[must_use]
    pub fn begin(&mut self, transaction: &CrimeCgiTransaction) -> bool {
        self.in_flight
            .insert((transaction.target, transaction.id), transaction.clone())
            .is_none()
    }

    /// Correlates one target completion with its controller.
    pub fn complete(
        &mut self,
        target: ComponentId,
        completion: CrimeCgiCompletion,
    ) -> Option<(ComponentId, CrimeCgiCompletion)> {
        let transaction = self.in_flight.remove(&(target, completion.id))?;
        Some((transaction.controller, completion))
    }

    /// Cancels all outstanding CGI requests.
    pub fn hard_reset(&mut self) {
        self.in_flight.clear();
    }

    /// Returns the number of outstanding requests.
    pub fn pending_transactions(&self) -> usize {
        self.in_flight.len()
    }
}

impl Component for CrimeCgiBus {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn reset(&mut self) {
        self.hard_reset();
    }
}

#[cfg(test)]
mod tests;
