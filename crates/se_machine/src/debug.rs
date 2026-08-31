//! Machine-specific debugger request and response envelopes.

use crate::indigo::ip12::debug::{
    DebugRequest as Ip12DebugRequest, DebugResponse as Ip12DebugResponse,
};

/// A side-effect-free debugger query for one machine model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DebugRequest {
    /// An Indigo IP12 debugger query.
    IndigoIp12(Ip12DebugRequest),
}

/// The result of one machine-specific debugger query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DebugResponse {
    /// An Indigo IP12 debugger response.
    IndigoIp12(Ip12DebugResponse),
}
