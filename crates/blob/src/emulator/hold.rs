//! Requests held at the emulator until a test lets them go, so a test
//! decides which of several requests in flight answers first, and how.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::op::S3Op;

/// How a held request ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Release {
    /// It takes effect and answers as usual.
    Land,
    /// It does nothing and answers `Unavailable`.
    Fail,
    /// It takes effect and then answers `Unavailable`, as a lost response.
    LandThenFail,
}

#[derive(Debug, Default)]
struct State {
    arrived: bool,
    release: Option<Release>,
}

/// One request kept waiting.
#[derive(Debug, Default)]
pub struct Hold {
    state: Mutex<State>,
    changed: Condvar,
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Hold {
    /// Whether the held request has reached the emulator, waiting at most
    /// `timeout` for it.
    pub fn wait_arrived(&self, timeout: Duration) -> bool {
        let deadline = Instant::now().checked_add(timeout);
        let mut state = lock(&self.state);
        while !state.arrived {
            let Some(left) = deadline.and_then(|d| d.checked_duration_since(Instant::now())) else {
                return false;
            };
            state = self
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }

    /// Lets the request go, ending as `how`. A request that has not arrived
    /// yet ends so as soon as it does.
    pub fn release(&self, how: Release) {
        lock(&self.state).release = Some(how);
        self.changed.notify_all();
    }

    /// Called by the request: says it arrived and waits to be released.
    pub(crate) fn hold(&self) -> Release {
        let mut state = lock(&self.state);
        state.arrived = true;
        self.changed.notify_all();
        // A test that never releases would pin this thread for good.
        let give_up = Instant::now().checked_add(MAX_HOLD);
        loop {
            if let Some(how) = state.release {
                return how;
            }
            let left = give_up.and_then(|d| d.checked_duration_since(Instant::now()));
            let Some(left) = left else {
                return Release::Fail;
            };
            state = self
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// How long a request waits for its release before it fails.
const MAX_HOLD: Duration = Duration::from_secs(60);

/// The holds waiting for the next requests of each kind.
#[derive(Debug)]
pub(crate) struct Holds(Mutex<[VecDeque<Arc<Hold>>; S3Op::ALL.len()]>);

impl Holds {
    pub(crate) fn new() -> Self {
        // ALLOC: test support; one empty queue a request kind.
        Self(Mutex::new(S3Op::ALL.map(|_| VecDeque::new())))
    }

    /// Holds for the next `n` requests of `op`, in the order they arrive.
    pub(crate) fn queue(&self, op: S3Op, n: u32) -> Vec<Arc<Hold>> {
        // ALLOC: test support; one hold a request a test asks to hold.
        let made: Vec<Arc<Hold>> = (0..n).map(|_| Arc::new(Hold::default())).collect();
        if let Some(q) = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(op.index())
        {
            // ALLOC: test support; the same holds, queued.
            q.extend(made.iter().cloned());
        }
        made
    }

    /// The hold the next request of `op` waits on, if one is queued.
    pub(crate) fn take(&self, op: S3Op) -> Option<Arc<Hold>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(op.index())
            .and_then(VecDeque::pop_front)
    }
}
