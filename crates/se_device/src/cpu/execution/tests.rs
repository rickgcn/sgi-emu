use std::collections::VecDeque;

use super::functional::FunctionalExecutor;
use super::protocol::{
    ExecutionAction, ExecutionCompletion, ExecutionTransactionId, FunctionalExecutorError,
    FunctionalExecutorState,
};
use super::target::{
    ExecutionBoundary, ExecutionTarget, ExecutionTargetAction, ExecutionTargetSignalAction,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
enum TestBoundary {
    Retired(u32),
}

impl ExecutionBoundary for TestBoundary {}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
enum TestAction {
    Transaction(u32),
    Boundary(u32),
    Idle,
    Failure,
}

#[derive(Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct TestError;

impl core::fmt::Display for TestError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "test target failure")
    }
}

impl std::error::Error for TestError {}

struct TestTarget {
    actions: VecDeque<TestAction>,
    signals: Vec<u32>,
    resets: usize,
}

impl TestTarget {
    fn new(actions: impl IntoIterator<Item = TestAction>) -> Self {
        Self {
            actions: actions.into_iter().collect(),
            signals: Vec::new(),
            resets: 0,
        }
    }

    fn next_action(&mut self) -> Result<ExecutionTargetAction<u32, TestBoundary>, TestError> {
        match self.actions.pop_front().unwrap() {
            TestAction::Transaction(value) => Ok(ExecutionTargetAction::Transaction(value)),
            TestAction::Boundary(value) => Ok(ExecutionTargetAction::Boundary(
                TestBoundary::Retired(value),
            )),
            TestAction::Idle => Ok(ExecutionTargetAction::Idle),
            TestAction::Failure => Err(TestError),
        }
    }
}

impl ExecutionTarget for TestTarget {
    type Transaction = u32;
    type Completion = u32;
    type Boundary = TestBoundary;
    type Signal = u32;
    type Error = TestError;

    fn reset(&mut self) {
        self.resets += 1;
    }

    fn signal(&mut self, signal: Self::Signal) -> ExecutionTargetSignalAction {
        self.signals.push(signal);
        ExecutionTargetSignalAction::Continue
    }

    fn begin(
        &mut self,
    ) -> Result<ExecutionTargetAction<Self::Transaction, Self::Boundary>, Self::Error> {
        self.next_action()
    }

    fn complete(
        &mut self,
        _completion: Self::Completion,
    ) -> Result<ExecutionTargetAction<Self::Transaction, Self::Boundary>, Self::Error> {
        self.next_action()
    }
}

#[test]
fn immediate_boundary_leaves_executor_ready() {
    let mut executor = FunctionalExecutor::new(TestTarget::new([TestAction::Boundary(7)]));

    assert_eq!(
        executor.poll(),
        Ok(ExecutionAction::Boundary(TestBoundary::Retired(7)))
    );
    assert_eq!(executor.state(), FunctionalExecutorState::Ready);
}

#[test]
fn idle_action_leaves_executor_ready_without_allocating_a_transaction() {
    let mut executor = FunctionalExecutor::new(TestTarget::new([
        TestAction::Idle,
        TestAction::Transaction(9),
        TestAction::Boundary(10),
    ]));

    assert_eq!(executor.poll(), Ok(ExecutionAction::Idle));
    assert_eq!(executor.state(), FunctionalExecutorState::Ready);
    let ExecutionAction::Transaction(transaction) = executor.poll().unwrap() else {
        panic!("expected transaction after idle");
    };
    assert_eq!(transaction.id, ExecutionTransactionId::new(0));
}

#[test]
fn transaction_waits_for_matching_completion() {
    let mut executor = FunctionalExecutor::new(TestTarget::new([
        TestAction::Transaction(11),
        TestAction::Boundary(12),
    ]));

    let ExecutionAction::Transaction(transaction) = executor.poll().unwrap() else {
        panic!("expected transaction");
    };
    assert_eq!(transaction.payload, 11);
    assert_eq!(
        executor.poll(),
        Ok(ExecutionAction::Waiting {
            transaction_id: transaction.id,
        })
    );

    executor
        .complete(ExecutionCompletion {
            id: transaction.id,
            payload: 99,
        })
        .unwrap();
    assert_eq!(
        executor.poll(),
        Ok(ExecutionAction::Boundary(TestBoundary::Retired(12)))
    );
}

#[test]
fn one_instruction_can_issue_multiple_sequential_transactions() {
    let mut executor = FunctionalExecutor::new(TestTarget::new([
        TestAction::Transaction(1),
        TestAction::Transaction(2),
        TestAction::Boundary(3),
    ]));

    let ExecutionAction::Transaction(first) = executor.poll().unwrap() else {
        panic!("expected first transaction");
    };
    executor
        .complete(ExecutionCompletion {
            id: first.id,
            payload: 10,
        })
        .unwrap();

    let ExecutionAction::Transaction(second) = executor.poll().unwrap() else {
        panic!("expected second transaction");
    };
    assert!(second.id > first.id);
    executor
        .complete(ExecutionCompletion {
            id: second.id,
            payload: 20,
        })
        .unwrap();
    assert_eq!(
        executor.poll(),
        Ok(ExecutionAction::Boundary(TestBoundary::Retired(3)))
    );
}

#[test]
fn mismatched_completion_is_rejected_without_losing_outstanding_transaction() {
    let mut executor = FunctionalExecutor::new(TestTarget::new([
        TestAction::Transaction(1),
        TestAction::Boundary(2),
    ]));
    let ExecutionAction::Transaction(transaction) = executor.poll().unwrap() else {
        panic!("expected transaction");
    };
    let wrong = ExecutionTransactionId::new(transaction.id.get() + 1);

    assert_eq!(
        executor.complete(ExecutionCompletion {
            id: wrong,
            payload: 0,
        }),
        Err(FunctionalExecutorError::MismatchedCompletion {
            expected: transaction.id,
            actual: wrong,
        })
    );
    assert_eq!(
        executor.state(),
        FunctionalExecutorState::Waiting {
            transaction_id: transaction.id,
        }
    );
}

#[test]
fn reset_rejects_old_completion_and_keeps_identifiers_monotonic() {
    let mut executor = FunctionalExecutor::new(TestTarget::new([
        TestAction::Transaction(1),
        TestAction::Transaction(2),
        TestAction::Boundary(3),
    ]));
    let ExecutionAction::Transaction(old) = executor.poll().unwrap() else {
        panic!("expected old transaction");
    };

    executor.reset();
    assert_eq!(executor.target().resets, 1);
    let ExecutionAction::Transaction(new) = executor.poll().unwrap() else {
        panic!("expected new transaction");
    };
    assert!(new.id > old.id);
    assert_eq!(
        executor.complete(ExecutionCompletion {
            id: old.id,
            payload: 0,
        }),
        Err(FunctionalExecutorError::MismatchedCompletion {
            expected: new.id,
            actual: old.id,
        })
    );
}

#[test]
fn unexpected_completion_is_rejected_while_ready() {
    let mut executor = FunctionalExecutor::new(TestTarget::new([TestAction::Boundary(1)]));
    let id = ExecutionTransactionId::new(44);

    assert_eq!(
        executor.complete(ExecutionCompletion { id, payload: 0 }),
        Err(FunctionalExecutorError::UnexpectedCompletion { completion_id: id })
    );
}

#[test]
fn target_failure_is_terminal_until_reset() {
    let mut executor = FunctionalExecutor::new(TestTarget::new([
        TestAction::Failure,
        TestAction::Boundary(5),
    ]));

    assert_eq!(
        executor.poll(),
        Err(FunctionalExecutorError::Target(TestError))
    );
    assert_eq!(executor.state(), FunctionalExecutorState::Failed);
    assert_eq!(executor.poll(), Err(FunctionalExecutorError::Failed));

    executor.reset();
    assert_eq!(
        executor.poll(),
        Ok(ExecutionAction::Boundary(TestBoundary::Retired(5)))
    );
}

#[test]
fn signals_are_forwarded_without_changing_executor_state() {
    let mut executor = FunctionalExecutor::new(TestTarget::new([TestAction::Boundary(1)]));

    executor.signal(7);
    executor.signal(8);

    assert_eq!(executor.target().signals, [7, 8]);
    assert_eq!(executor.state(), FunctionalExecutorState::Ready);
}
