//! Functional execution target interface.

/// Marker implemented by an ISA-specific architectural execution boundary.
pub trait ExecutionBoundary {}

/// Action produced while executing one architectural instruction.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum ExecutionTargetAction<T, B> {
    /// Internal work completed without reaching an architectural boundary.
    Continue,

    /// An external transaction must complete before execution can continue.
    Transaction(T),

    /// The current architectural instruction reached its commit or exception boundary.
    Boundary(B),

    /// Execution is quiescent without an outstanding transaction.
    Idle,
}

/// Effect of an asynchronous signal on an outstanding transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum ExecutionTargetSignalAction {
    /// Preserve the current executor state and any outstanding transaction.
    Continue,

    /// Cancel any outstanding transaction and resume by polling the target.
    CancelPending,
}

/// ISA-specific target driven by the functional executor.
pub trait ExecutionTarget {
    /// External transaction payload produced by the target.
    type Transaction;

    /// External completion payload consumed by the target.
    type Completion;

    /// Architectural boundary reported after an instruction commits or takes an exception.
    type Boundary: ExecutionBoundary;

    /// Asynchronous signal accepted by the target.
    type Signal;

    /// Internal target failure. Architectural exceptions must not use this type.
    type Error;

    /// Resets the target to its deterministic initial state.
    fn reset(&mut self);

    /// Delivers an asynchronous signal to the target.
    fn signal(&mut self, signal: Self::Signal) -> ExecutionTargetSignalAction;

    /// Begins execution at an architectural instruction boundary.
    fn begin(
        &mut self,
    ) -> Result<ExecutionTargetAction<Self::Transaction, Self::Boundary>, Self::Error>;

    /// Continues the current instruction with an external completion.
    fn complete(
        &mut self,
        completion: Self::Completion,
    ) -> Result<ExecutionTargetAction<Self::Transaction, Self::Boundary>, Self::Error>;
}
