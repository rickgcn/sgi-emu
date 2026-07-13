//! Functional execution request/completion protocol.

use core::fmt;

/// Identifier correlating one external transaction with its completion.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub struct ExecutionTransactionId(u128);

impl ExecutionTransactionId {
    /// Creates an identifier from its raw value.
    pub const fn new(value: u128) -> Self {
        Self(value)
    }

    /// Returns the raw identifier value.
    pub const fn get(self) -> u128 {
        self.0
    }
}

impl fmt::Display for ExecutionTransactionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "execution-transaction:{}", self.0)
    }
}

/// External transaction emitted by a functional executor.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ExecutionTransaction<T> {
    /// Correlation identifier.
    pub id: ExecutionTransactionId,

    /// Target-defined transaction payload.
    pub payload: T,
}

/// External completion delivered to a functional executor.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ExecutionCompletion<C> {
    /// Identifier copied from the completed transaction.
    pub id: ExecutionTransactionId,

    /// Target-defined completion payload.
    pub payload: C,
}

/// Observable functional executor state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum FunctionalExecutorState {
    /// The executor may begin or continue an instruction.
    Ready,

    /// The executor is waiting for one correlated external completion.
    Waiting {
        /// Outstanding transaction identifier.
        transaction_id: ExecutionTransactionId,
    },

    /// The execution target or executor encountered a terminal internal failure.
    Failed,
}

/// Result of polling a functional executor.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum ExecutionAction<T, B> {
    /// Route this transaction through the CPU's bus controller role.
    Transaction(ExecutionTransaction<T>),

    /// One architectural instruction reached a commit or exception boundary.
    Boundary(B),

    /// The target is quiescent and may be polled again after an external event.
    Idle,

    /// Execution cannot continue until the outstanding transaction completes.
    Waiting {
        /// Outstanding transaction identifier.
        transaction_id: ExecutionTransactionId,
    },
}

/// Internal functional executor failure.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum FunctionalExecutorError<E> {
    /// A completion arrived while no transaction was outstanding.
    UnexpectedCompletion {
        /// Completion identifier.
        completion_id: ExecutionTransactionId,
    },

    /// A completion did not match the outstanding transaction.
    MismatchedCompletion {
        /// Outstanding transaction identifier.
        expected: ExecutionTransactionId,

        /// Received completion identifier.
        actual: ExecutionTransactionId,
    },

    /// The transaction identifier space was exhausted.
    TransactionIdOverflow,

    /// The execution target reported an internal failure.
    Target(E),

    /// The executor was polled after entering its terminal failed state.
    Failed,
}

impl<E> fmt::Display for FunctionalExecutorError<E>
where
    E: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedCompletion { completion_id } => {
                write!(f, "unexpected completion for {completion_id}")
            }
            Self::MismatchedCompletion { expected, actual } => {
                write!(
                    f,
                    "completion {actual} does not match outstanding {expected}"
                )
            }
            Self::TransactionIdOverflow => write!(f, "execution transaction identifier overflow"),
            Self::Target(error) => write!(f, "execution target failed: {error}"),
            Self::Failed => write!(f, "functional executor is in the failed state"),
        }
    }
}

impl<E> std::error::Error for FunctionalExecutorError<E> where E: std::error::Error + 'static {}
