//! Machine-level debugger request and response envelopes.

use crate::indigo::ip12::debug::{
    DebugRequest as Ip12DebugRequest, DebugResponse as Ip12DebugResponse,
};

/// A side-effect-free debugger query for a configured machine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DebugRequest {
    /// Computes a machine-defined fingerprint for detecting execution
    /// divergence.
    MachineStateFingerprint,
    /// An Indigo IP12 debugger query.
    IndigoIp12(Ip12DebugRequest),
}

/// The result of one machine-level debugger query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DebugResponse {
    /// A machine-defined fingerprint for detecting execution divergence.
    ///
    /// The fingerprint is not a complete machine save state and may omit
    /// state that has not yet affected processor-visible execution.
    MachineStateFingerprint([u8; 32]),
    /// An Indigo IP12 debugger response.
    IndigoIp12(Ip12DebugResponse),
}
