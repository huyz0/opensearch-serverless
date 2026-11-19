//! Running one call per shard concurrently, a bounded number at a time, and
//! reporting which shards answered.
//!
//! The caller's own thread is one of the workers and every worker takes the
//! next item the moment it finishes one, so a slow shard holds up only its
//! own worker, and a refused or late helper costs parallelism, never
//! progress.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

/// Why one item of a fan-out gave no answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskError<E> {
    /// The call returned an error.
    Failed(E),
    /// The call panicked.
    Panicked,
}

/// How completely a fan-out was answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Coverage {
    /// Every shard, asked or skipped.
    pub total: usize,
    /// The shards that answered, plus those skipped.
    pub successful: usize,
    /// The shards ruled out before the fan-out.
    pub skipped: usize,
    /// The shards that gave no answer.
    pub failed: usize,
}

/// What a fan-out gathered.
#[derive(Debug, PartialEq, Eq)]
pub struct Gathered<T, E> {
    /// The answers, in item order.
    pub answers: Vec<T>,
    /// The items that gave none, by item index.
    pub failures: Vec<(usize, TaskError<E>)>,
    /// How completely the shards answered.
    pub coverage: Coverage,
}

/// One item's slot: empty until a worker has run it.
type Slot<T, E> = Mutex<Option<Result<T, TaskError<E>>>>;

/// Calls `call` on every item, at most `concurrency` at a time (at least
/// one), and returns each item's outcome in item order.
pub fn run<I, T, E, F>(concurrency: usize, items: &[I], call: F) -> Vec<Result<T, TaskError<E>>>
where
    I: Sync,
    T: Send,
    E: Send,
    F: Fn(&I) -> Result<T, E> + Sync,
{
    // ALLOC: one slot per item, bounded by the shards of the request.
    let slots: Vec<Slot<T, E>> = items.iter().map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let helpers = concurrency.max(1).min(items.len()).saturating_sub(1);
    thread::scope(|scope| {
        for _ in 0..helpers {
            scope.spawn(|| drain(&next, items, &slots, &call));
        }
        drain(&next, items, &slots, &call);
    });
    slots
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .unwrap_or(Err(TaskError::Panicked))
        })
        // ALLOC: one outcome per item.
        .collect()
}

/// Takes and runs items until none are left.
fn drain<I, T, E, F>(next: &AtomicUsize, items: &[I], slots: &[Slot<T, E>], call: &F)
where
    F: Fn(&I) -> Result<T, E>,
{
    loop {
        let at = next.fetch_add(1, Ordering::Relaxed);
        let (Some(item), Some(slot)) = (items.get(at), slots.get(at)) else {
            return;
        };
        let outcome = match catch_unwind(AssertUnwindSafe(|| call(item))) {
            Ok(Ok(answer)) => Ok(answer),
            Ok(Err(error)) => Err(TaskError::Failed(error)),
            Err(_) => Err(TaskError::Panicked),
        };
        *slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(outcome);
    }
}

/// Splits `outcomes` into answers and failures with the coverage they give,
/// `skipped` shards having been ruled out before the fan-out.
///
/// # Errors
///
/// When there were items and none answered, the failure of the lowest item,
/// with its index: a search no shard answered has no coverage to report.
pub fn gather<T, E>(
    skipped: usize,
    outcomes: Vec<Result<T, TaskError<E>>>,
) -> Result<Gathered<T, E>, (usize, TaskError<E>)> {
    let asked = outcomes.len();
    // ALLOC: at most one answer or failure per item.
    let mut answers = Vec::with_capacity(asked);
    let mut failures = Vec::new();
    for (index, outcome) in outcomes.into_iter().enumerate() {
        match outcome {
            Ok(answer) => answers.push(answer),
            Err(error) => failures.push((index, error)),
        }
    }
    if answers.is_empty() && !failures.is_empty() {
        return Err(failures.swap_remove(0));
    }
    let failed = failures.len();
    Ok(Gathered {
        coverage: Coverage {
            total: asked.saturating_add(skipped),
            successful: answers.len().saturating_add(skipped),
            skipped,
            failed,
        },
        answers,
        failures,
    })
}
