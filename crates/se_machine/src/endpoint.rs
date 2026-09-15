//! Stable identities and typed capabilities of a configured machine's I/O.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Stable identity of one runtime I/O role in a machine topology.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct EndpointKey(String);

impl EndpointKey {
    pub(crate) fn new(value: &str) -> Self {
        Self(value.into())
    }

    /// Returns the opaque identity for transport and display equality checks.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Frontend-visible payload family of an endpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EndpointKind {
    /// A byte-oriented serial connection.
    Serial,
    /// Physical keyboard transitions.
    Keyboard,
    /// Relative pointer movement and buttons.
    Pointer,
    /// Ethernet frames.
    Ethernet,
    /// Video display states.
    Video,
}

/// Directions in which an endpoint carries data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EndpointDirection {
    /// Host to machine.
    Input,
    /// Machine to host.
    Output,
    /// Both host to machine and machine to host.
    Bidirectional,
}

impl EndpointDirection {
    /// Reports whether the endpoint accepts host input.
    #[must_use]
    pub const fn accepts_input(self) -> bool {
        matches!(self, Self::Input | Self::Bidirectional)
    }

    /// Reports whether the endpoint may publish machine output.
    #[must_use]
    pub const fn accepts_output(self) -> bool {
        matches!(self, Self::Output | Self::Bidirectional)
    }
}

/// One frontend-visible capability of a configured machine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EndpointDescriptor {
    key: EndpointKey,
    label: String,
    kind: EndpointKind,
    direction: EndpointDirection,
}

impl EndpointDescriptor {
    pub(crate) fn new(
        key: EndpointKey,
        label: &str,
        kind: EndpointKind,
        direction: EndpointDirection,
    ) -> Self {
        Self {
            key,
            label: label.into(),
            kind,
            direction,
        }
    }

    /// Returns the stable machine endpoint identity.
    #[must_use]
    pub const fn key(&self) -> &EndpointKey {
        &self.key
    }

    /// Returns the user-visible endpoint name.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Returns the endpoint payload family.
    #[must_use]
    pub const fn kind(&self) -> EndpointKind {
        self.kind
    }

    /// Returns the endpoint direction.
    #[must_use]
    pub const fn direction(&self) -> EndpointDirection {
        self.direction
    }
}

/// Duplicate endpoint identity in one machine catalog.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DuplicateEndpointKey(EndpointKey);

impl fmt::Display for DuplicateEndpointKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "duplicate machine endpoint key: {}",
            self.0.as_str()
        )
    }
}

impl Error for DuplicateEndpointKey {}

/// Canonically ordered frontend I/O capabilities of one machine topology.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EndpointCatalog {
    endpoints: Vec<EndpointDescriptor>,
}

impl EndpointCatalog {
    /// Validates unique keys while retaining machine-defined endpoint order.
    ///
    /// # Errors
    ///
    /// Returns [`DuplicateEndpointKey`] if one key occurs more than once.
    pub fn try_new(endpoints: Vec<EndpointDescriptor>) -> Result<Self, DuplicateEndpointKey> {
        let mut seen = BTreeSet::new();
        for endpoint in &endpoints {
            if !seen.insert(endpoint.key.clone()) {
                return Err(DuplicateEndpointKey(endpoint.key.clone()));
            }
        }
        Ok(Self { endpoints })
    }

    /// Returns descriptors in stable machine-defined order.
    #[must_use]
    pub fn endpoints(&self) -> &[EndpointDescriptor] {
        &self.endpoints
    }

    /// Finds one endpoint by exact opaque identity.
    #[must_use]
    pub fn get(&self, key: &EndpointKey) -> Option<&EndpointDescriptor> {
        self.endpoints.iter().find(|endpoint| endpoint.key() == key)
    }
}
