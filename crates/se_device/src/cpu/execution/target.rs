//! Functional execution target interface.

/// Marker implemented by an ISA-specific architectural execution boundary.
pub trait ExecutionBoundary {}

/// Action produced while executing one architectural instruction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionTargetAction<T, B> {
    /// An external transaction must complete before execution can continue.
    Transaction(T),

    /// The current architectural instruction reached its commit or exception boundary.
    Boundary(B),

    /// Execution is quiescent without an outstanding transaction.
    Idle,
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
    fn signal(&mut self, signal: Self::Signal);

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
