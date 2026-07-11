//! ISA-independent functional executor state machine.

use super::protocol::{
    ExecutionAction, ExecutionCompletion, ExecutionTransaction, ExecutionTransactionId,
    FunctionalExecutorError, FunctionalExecutorState,
};
use super::target::{ExecutionTarget, ExecutionTargetAction, ExecutionTargetSignalAction};

/// Poll result produced by a functional executor.
pub type FunctionalExecutorPoll<T> = Result<
    ExecutionAction<<T as ExecutionTarget>::Transaction, <T as ExecutionTarget>::Boundary>,
    FunctionalExecutorError<<T as ExecutionTarget>::Error>,
>;

/// Functional, one-instruction-at-a-time CPU executor.
pub struct FunctionalExecutor<T>
where
    T: ExecutionTarget,
{
    target: T,
    state: FunctionalExecutorState,
    next_transaction_id: u128,
    queued_action: Option<ExecutionTargetAction<T::Transaction, T::Boundary>>,
}

impl<T> FunctionalExecutor<T>
where
    T: ExecutionTarget,
{
    /// Creates an executor around an ISA-specific target.
    pub const fn new(target: T) -> Self {
        Self {
            target,
            state: FunctionalExecutorState::Ready,
            next_transaction_id: 0,
            queued_action: None,
        }
    }

    /// Returns the current executor state.
    pub const fn state(&self) -> FunctionalExecutorState {
        self.state
    }

    /// Returns an immutable reference to the execution target.
    pub const fn target(&self) -> &T {
        &self.target
    }

    /// Returns a mutable reference to the execution target.
    pub const fn target_mut(&mut self) -> &mut T {
        &mut self.target
    }

    /// Consumes the executor and returns its execution target.
    pub fn into_target(self) -> T {
        self.target
    }

    /// Resets execution while retaining the monotonic transaction identifier sequence.
    pub fn reset(&mut self) {
        self.target.reset();
        self.state = FunctionalExecutorState::Ready;
        self.queued_action = None;
    }

    /// Delivers an asynchronous signal without interpreting its ISA-specific meaning.
    pub fn signal(&mut self, signal: T::Signal) {
        if matches!(
            self.target.signal(signal),
            ExecutionTargetSignalAction::CancelPending
        ) {
            self.state = FunctionalExecutorState::Ready;
            self.queued_action = None;
        }
    }

    /// Polls for the next transaction, architectural boundary, idle state, or wait state.
    pub fn poll(&mut self) -> FunctionalExecutorPoll<T> {
        match self.state {
            FunctionalExecutorState::Waiting { transaction_id } => {
                return Ok(ExecutionAction::Waiting { transaction_id });
            }
            FunctionalExecutorState::Failed => return Err(FunctionalExecutorError::Failed),
            FunctionalExecutorState::Ready => {}
        }

        let action = match self.queued_action.take() {
            Some(action) => action,
            None => self.target.begin().map_err(|error| {
                self.state = FunctionalExecutorState::Failed;
                FunctionalExecutorError::Target(error)
            })?,
        };

        self.publish(action)
    }

    /// Delivers a correlated external completion to the execution target.
    pub fn complete(
        &mut self,
        completion: ExecutionCompletion<T::Completion>,
    ) -> Result<(), FunctionalExecutorError<T::Error>> {
        let expected = match self.state {
            FunctionalExecutorState::Waiting { transaction_id } => transaction_id,
            FunctionalExecutorState::Ready => {
                return Err(FunctionalExecutorError::UnexpectedCompletion {
                    completion_id: completion.id,
                });
            }
            FunctionalExecutorState::Failed => return Err(FunctionalExecutorError::Failed),
        };

        if completion.id != expected {
            return Err(FunctionalExecutorError::MismatchedCompletion {
                expected,
                actual: completion.id,
            });
        }

        match self.target.complete(completion.payload) {
            Ok(action) => {
                self.state = FunctionalExecutorState::Ready;
                self.queued_action = Some(action);
                Ok(())
            }
            Err(error) => {
                self.state = FunctionalExecutorState::Failed;
                self.queued_action = None;
                Err(FunctionalExecutorError::Target(error))
            }
        }
    }

    fn publish(
        &mut self,
        action: ExecutionTargetAction<T::Transaction, T::Boundary>,
    ) -> FunctionalExecutorPoll<T> {
        match action {
            ExecutionTargetAction::Transaction(payload) => {
                let id = self.allocate_transaction_id()?;
                self.state = FunctionalExecutorState::Waiting { transaction_id: id };
                Ok(ExecutionAction::Transaction(ExecutionTransaction {
                    id,
                    payload,
                }))
            }
            ExecutionTargetAction::Boundary(boundary) => {
                self.state = FunctionalExecutorState::Ready;
                Ok(ExecutionAction::Boundary(boundary))
            }
            ExecutionTargetAction::Idle => {
                self.state = FunctionalExecutorState::Ready;
                Ok(ExecutionAction::Idle)
            }
        }
    }

    fn allocate_transaction_id(
        &mut self,
    ) -> Result<ExecutionTransactionId, FunctionalExecutorError<T::Error>> {
        let Some(next_transaction_id) = self.next_transaction_id.checked_add(1) else {
            self.state = FunctionalExecutorState::Failed;
            return Err(FunctionalExecutorError::TransactionIdOverflow);
        };

        let id = ExecutionTransactionId::new(self.next_transaction_id);
        self.next_transaction_id = next_transaction_id;
        Ok(id)
    }
}
