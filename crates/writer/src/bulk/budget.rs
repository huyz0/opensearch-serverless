//! `BulkMemoryBudget`: the heap a node's streamed bulk slices may hold at
//! once, granted first come, first served. A slice that does not fit waits
//! its turn (its body is not read meanwhile, so TCP slows the client), and
//! only one that has waited past the timeout is refused, with a 429.
//! Besides bytes, at most so many holders at once: more slices applying
//! than cores finishes none sooner and only holds more heap. A request
//! larger than the whole budget gets the whole budget at the front.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use oss_rest::{Exception, RestError};

#[derive(Debug, Default)]
struct State {
    used: usize,
    holders: usize,
    /// Tickets waiting, front first.
    queue: VecDeque<u64>,
    next_ticket: u64,
}

#[derive(Debug)]
struct Inner {
    capacity: usize,
    max_holders: usize,
    state: Mutex<State>,
    turn: Condvar,
}

/// A node's bulk budget; clones share it.
#[derive(Clone, Debug)]
pub struct BulkBudget {
    inner: Arc<Inner>,
}

/// Bytes held from the budget, given back on drop.
#[derive(Debug)]
pub struct Grant {
    inner: Arc<Inner>,
    bytes: usize,
}

impl Grant {
    /// The bytes held.
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

impl Drop for Grant {
    fn drop(&mut self) {
        let mut s = lock(&self.inner.state);
        s.used = s.used.saturating_sub(self.bytes);
        s.holders = s.holders.saturating_sub(1);
        drop(s);
        self.inner.turn.notify_all();
    }
}

/// A poisoned lock still holds consistent counts: every update is a few
/// arithmetic steps that cannot panic.
fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl BulkBudget {
    /// A budget of `capacity` bytes and at most `max_holders` grants at
    /// once (both at least 1).
    pub fn new(capacity: usize, max_holders: usize) -> Self {
        Self {
            inner: Arc::new(Inner {
                capacity: capacity.max(1),
                max_holders: max_holders.max(1),
                state: Mutex::new(State::default()),
                turn: Condvar::new(),
            }),
        }
    }

    /// The bytes a request for `bytes` is granted: what it asked for, or the
    /// whole budget if it asked for more.
    pub fn granted(&self, bytes: usize) -> usize {
        bytes.min(self.inner.capacity)
    }

    /// The bytes and holders granted now.
    pub fn in_use(&self) -> (usize, usize) {
        let s = lock(&self.inner.state);
        (s.used, s.holders)
    }

    /// Waits for `bytes`, in arrival order, for at most `wait`.
    ///
    /// # Errors
    ///
    /// A 429 `rejected_execution_exception` when the wait passes first.
    pub fn acquire(&self, bytes: usize, wait: Duration) -> Result<Grant, RestError> {
        let want = self.granted(bytes);
        let deadline = Instant::now().checked_add(wait);
        let inner = &self.inner;
        let mut s = lock(&inner.state);
        let ticket = s.next_ticket;
        s.next_ticket = s.next_ticket.wrapping_add(1);
        // ALLOC: one ticket a waiting slice; at most the requests in flight.
        s.queue.push_back(ticket);
        loop {
            let front = s.queue.front() == Some(&ticket);
            let fits =
                s.used.saturating_add(want) <= inner.capacity && s.holders < inner.max_holders;
            if front && fits {
                s.queue.pop_front();
                s.used = s.used.saturating_add(want);
                s.holders = s.holders.saturating_add(1);
                drop(s);
                // The next in line may fit too.
                inner.turn.notify_all();
                return Ok(Grant {
                    inner: Arc::clone(inner),
                    bytes: want,
                });
            }
            let left = deadline.map(|d| d.saturating_duration_since(Instant::now()));
            if left == Some(Duration::ZERO) {
                s.queue.retain(|&t| t != ticket);
                drop(s);
                inner.turn.notify_all();
                return Err(RestError::exception(
                    429,
                    Exception::new(
                        "rejected_execution_exception",
                        // ALLOC: error path; one message.
                        format!(
                            "a bulk slice of [{want}] bytes waited [{}ms] for the node's bulk memory budget of [{}] bytes",
                            wait.as_millis(),
                            inner.capacity
                        ),
                    ),
                ));
            }
            s = match left {
                Some(d) => {
                    inner
                        .turn
                        .wait_timeout(s, d)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
                None => inner.turn.wait(s).unwrap_or_else(PoisonError::into_inner),
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;

    const LONG: Duration = Duration::from_secs(10);

    #[test]
    fn grants_fit_and_come_back() {
        let b = BulkBudget::new(100, 4);
        let g = b.acquire(60, LONG).unwrap();
        assert_eq!((g.bytes(), b.in_use()), (60, (60, 1)));
        let h = b.acquire(40, LONG).unwrap();
        assert_eq!(b.in_use(), (100, 2));
        drop(g);
        drop(h);
        assert_eq!(b.in_use(), (0, 0));
    }

    #[test]
    fn an_oversized_request_gets_the_whole_budget() {
        let b = BulkBudget::new(100, 4);
        assert_eq!(b.granted(500), 100);
        let g = b.acquire(500, LONG).unwrap();
        assert_eq!(g.bytes(), 100);
    }

    #[test]
    fn a_slice_that_waits_past_the_timeout_is_refused_with_429() {
        let b = BulkBudget::new(100, 4);
        let _g = b.acquire(100, LONG).unwrap();
        let e = b.acquire(1, Duration::from_millis(20)).unwrap_err();
        assert_eq!(e.status(), 429);
        assert_eq!(
            e.as_exception().map(|x| x.kind().to_owned()),
            Some("rejected_execution_exception".to_owned())
        );
        // The refused ticket left the queue: the next one is granted.
        drop(_g);
        assert!(b.acquire(100, Duration::from_millis(20)).is_ok());
    }

    #[test]
    fn holders_are_bounded() {
        let b = BulkBudget::new(100, 1);
        let g = b.acquire(1, LONG).unwrap();
        assert!(b.acquire(1, Duration::from_millis(20)).is_err());
        drop(g);
        assert!(b.acquire(1, Duration::from_millis(20)).is_ok());
    }

    #[test]
    fn grants_come_in_arrival_order() {
        let b = BulkBudget::new(100, 4);
        let first = b.acquire(100, LONG).unwrap();
        let (tx, rx) = mpsc::sync_channel(2);
        let mut waiters = Vec::new();
        // A large request queued before a small one is served first, though
        // the small one would fit sooner.
        for (name, bytes) in [("large", 100usize), ("small", 1)] {
            let (b, tx) = (b.clone(), tx.clone());
            waiters.push(thread::spawn(move || {
                let g = b.acquire(bytes, LONG).unwrap();
                tx.send(name).unwrap();
                thread::sleep(Duration::from_millis(20));
                drop(g);
            }));
            // Let it queue before the next one.
            let patience = Instant::now() + Duration::from_secs(5);
            while lock(b_state(&first)).queue.len() < waiters.len() {
                assert!(Instant::now() < patience, "the waiter never queued");
                thread::yield_now();
            }
        }
        drop(first);
        let order: Vec<&str> = rx.iter().take(2).collect();
        assert_eq!(order, ["large", "small"]);
        for w in waiters {
            w.join().unwrap();
        }
    }

    fn b_state(g: &Grant) -> &Mutex<State> {
        &g.inner.state
    }
}
