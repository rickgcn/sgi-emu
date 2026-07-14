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
#[derive(Clone, serde::Deserialize, serde::Serialize)]
#[serde(bound(
    serialize = "T: serde::Serialize, T::Transaction: serde::Serialize, T::Boundary: serde::Serialize",
    deserialize = "T: serde::Deserialize<'de>, T::Transaction: serde::Deserialize<'de>, T::Boundary: serde::Deserialize<'de>"
))]
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

    /// Returns whether no transaction or completed target action is pending.
    pub const fn ready_for_direct_execution(&self) -> bool {
        matches!(self.state, FunctionalExecutorState::Ready)
            && matches!(
                self.queued_action,
                None | Some(ExecutionTargetAction::Continue)
            )
    }

    /// Consumes a queued internal continuation at a dispatcher-safe point.
    pub fn consume_ready_continuation(&mut self) -> bool {
        if !matches!(self.state, FunctionalExecutorState::Ready)
            || !matches!(self.queued_action, Some(ExecutionTargetAction::Continue))
        {
            return false;
        }
        self.queued_action = None;
        true
    }

    /// Publishes a target action produced by an ISA-specific accelerated path.
    ///
    /// Callers must first observe [`Self::ready_for_direct_execution`]. Transaction
    /// identifiers continue to be allocated exclusively by this executor.
    pub fn publish_ready_action(
        &mut self,
        action: ExecutionTargetAction<T::Transaction, T::Boundary>,
    ) -> FunctionalExecutorPoll<T> {
        if !self.ready_for_direct_execution() {
            return Err(FunctionalExecutorError::Failed);
        }
        self.publish(action)
    }

    /// Publishes one accelerated transaction without an intermediate action enum.
    pub fn publish_ready_transaction(
        &mut self,
        payload: T::Transaction,
    ) -> Result<ExecutionTransaction<T::Transaction>, FunctionalExecutorError<T::Error>> {
        if !self.ready_for_direct_execution() {
            return Err(FunctionalExecutorError::Failed);
        }
        let id = self.allocate_transaction_id()?;
        self.state = FunctionalExecutorState::Waiting { transaction_id: id };
        Ok(ExecutionTransaction { id, payload })
    }

    /// Accounts for side-effect-free transactions elided by an accelerated path.
    ///
    /// The identifiers are consumed in architectural transaction order without
    /// publishing work to the external protocol.
    pub fn account_ready_transactions(
        &mut self,
        transactions: u64,
    ) -> Result<(), FunctionalExecutorError<T::Error>> {
        if !self.ready_for_direct_execution() {
            return Err(FunctionalExecutorError::Failed);
        }
        let Some(next_transaction_id) = self
            .next_transaction_id
            .checked_add(u128::from(transactions))
        else {
            self.state = FunctionalExecutorState::Failed;
            return Err(FunctionalExecutorError::TransactionIdOverflow);
        };
        self.next_transaction_id = next_transaction_id;
        Ok(())
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

        loop {
            let action = match self.queued_action.take() {
                Some(action) => action,
                None => self.target.begin().map_err(|error| {
                    self.state = FunctionalExecutorState::Failed;
                    FunctionalExecutorError::Target(error)
                })?,
            };
            if matches!(action, ExecutionTargetAction::Continue) {
                self.state = FunctionalExecutorState::Ready;
                continue;
            }
            return self.publish(action);
        }
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
            ExecutionTargetAction::Continue => {
                self.state = FunctionalExecutorState::Ready;
                self.poll()
            }
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
