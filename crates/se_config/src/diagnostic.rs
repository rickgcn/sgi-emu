//! Problems found while resolving a draft.

use crate::id::{NodeId, PropertyId};

/// The severity of a configuration problem.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticSeverity {
    /// A condition the user should review.
    Warning,
    /// An invalid configuration selection.
    Error,
}

/// The location to which a diagnostic applies.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiagnosticTarget {
    /// The configuration as a whole.
    Global,
    /// A topology node or a slot ID supplied by the draft.
    Node(NodeId),
    /// A property ID supplied by the draft.
    Property(PropertyId),
}

/// A machine-readable problem code and a user-readable explanation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    /// Whether the problem is a warning or an error.
    pub severity: DiagnosticSeverity,
    /// A stable, machine-readable problem identifier.
    pub code: String,
    /// The node, property, or whole configuration affected.
    pub target: DiagnosticTarget,
    /// A user-readable description of the problem.
    pub message: String,
}
