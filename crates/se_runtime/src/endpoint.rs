//! Runtime identity and typed output for live machine endpoints.

use se_machine::endpoint::{EndpointDirection, EndpointKey, EndpointKind};
use se_machine::output::VideoOutput;

/// An endpoint identity bound to one installed machine instance.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EndpointHandle {
    generation: u64,
    key: EndpointKey,
}

impl EndpointHandle {
    pub(crate) const fn new(generation: u64, key: EndpointKey) -> Self {
        Self { generation, key }
    }

    /// Returns the installed machine generation.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Returns the opaque machine endpoint identity.
    #[must_use]
    pub const fn key(&self) -> &EndpointKey {
        &self.key
    }
}

/// One endpoint visible to a frontend at a coherent runtime boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeEndpointDescriptor {
    handle: EndpointHandle,
    label: String,
    kind: EndpointKind,
    direction: EndpointDirection,
}

impl RuntimeEndpointDescriptor {
    pub(crate) fn new(
        handle: EndpointHandle,
        label: &str,
        kind: EndpointKind,
        direction: EndpointDirection,
    ) -> Self {
        Self {
            handle,
            label: label.into(),
            kind,
            direction,
        }
    }

    /// Returns the live endpoint handle.
    #[must_use]
    pub const fn handle(&self) -> &EndpointHandle {
        &self.handle
    }

    /// Returns the user-visible endpoint label.
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

/// Atomically sampled endpoints of the active machine generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeEndpointCatalog {
    generation: u64,
    endpoints: Vec<RuntimeEndpointDescriptor>,
}

impl RuntimeEndpointCatalog {
    pub(crate) const fn new(generation: u64, endpoints: Vec<RuntimeEndpointDescriptor>) -> Self {
        Self {
            generation,
            endpoints,
        }
    }

    /// Returns the active machine generation.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Returns endpoints in stable machine service order.
    #[must_use]
    pub fn endpoints(&self) -> &[RuntimeEndpointDescriptor] {
        &self.endpoints
    }
}

/// Typed frontend output of one live endpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeOutputPayload {
    /// Serial bytes emitted in order.
    Serial(Vec<u8>),
    /// The newest complete video state.
    Video(VideoOutput),
}

/// Frontend output tagged with the exact live endpoint handle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeOutput {
    handle: EndpointHandle,
    payload: RuntimeOutputPayload,
}

impl RuntimeOutput {
    pub(crate) const fn new(handle: EndpointHandle, payload: RuntimeOutputPayload) -> Self {
        Self { handle, payload }
    }

    /// Returns the emitting live endpoint.
    #[must_use]
    pub const fn handle(&self) -> &EndpointHandle {
        &self.handle
    }

    /// Returns typed output data.
    #[must_use]
    pub const fn payload(&self) -> &RuntimeOutputPayload {
        &self.payload
    }
}
