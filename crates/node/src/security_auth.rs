//! Who a request is: HTTP basic credentials checked against the snapshot,
//! with a bounded bcrypt check ([ADR 0043](../../../docs/adr/0043-security-configuration-as-document-sets.md)).

use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use oss_coordinator::security::basic;
use oss_rest::{Admission, Denial, Guard, Verdict};

use crate::security_state::{Clock, SecurityState};

/// The most password checks that run at once; the next is refused.
pub const MAX_VERIFICATIONS: usize = 4;
/// Checks remembered as passed.
pub const MAX_CACHED: usize = 4096;
/// Wrong passwords for one user within the window before it is throttled.
pub const MAX_FAILURES: u32 = 5;
/// The failure window.
pub const FAILURE_WINDOW_MILLIS: u64 = 10_000;
/// Users whose failures are remembered.
const MAX_TRACKED: usize = 4096;

const TEXT: &str = "text/plain; charset=UTF-8";

/// Why a request is not admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// No credentials, a wrong password or an unknown user: all one answer.
    Unauthorized,
    /// Too many wrong passwords for the user, or every check slot busy.
    Throttled,
    /// The configuration cannot be read.
    Unavailable,
}

struct Passed {
    hash: String,
}

/// Checks credentials against a [`SecurityState`].
pub struct Authenticator {
    state: Arc<SecurityState>,
    keys: RandomState,
    passed: Mutex<HashMap<(String, u64), Passed>>,
    failures: Mutex<HashMap<String, (u64, u32)>>,
    running: AtomicUsize,
    slots: usize,
    capacity: usize,
    clock: Clock,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A check slot, given back on drop.
struct Slot<'a>(&'a AtomicUsize);

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Authenticator {
    /// An authenticator over `state`.
    pub fn new(state: Arc<SecurityState>, clock: Clock) -> Self {
        Self {
            state,
            keys: RandomState::new(),
            passed: Mutex::new(HashMap::new()),
            failures: Mutex::new(HashMap::new()),
            running: AtomicUsize::new(0),
            slots: MAX_VERIFICATIONS,
            capacity: MAX_CACHED,
            clock,
        }
    }

    /// Limits the checks that run at once to `slots` and the remembered
    /// passes to `capacity`, in place of the defaults.
    #[must_use]
    pub fn with_limits(mut self, slots: usize, capacity: usize) -> Self {
        self.slots = slots;
        self.capacity = capacity;
        self
    }

    /// How many passed checks are remembered.
    pub fn remembered(&self) -> usize {
        lock(&self.passed).len()
    }

    fn slot(&self) -> Option<Slot<'_>> {
        let before = self.running.fetch_add(1, Ordering::SeqCst);
        if before >= self.slots {
            self.running.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Slot(&self.running))
    }

    fn throttled(&self, user: &str, now: u64) -> bool {
        lock(&self.failures).get(user).is_some_and(|(since, n)| {
            now.saturating_sub(*since) < FAILURE_WINDOW_MILLIS && *n >= MAX_FAILURES
        })
    }

    fn failed(&self, user: &str, now: u64) {
        let mut failures = lock(&self.failures);
        if failures.len() >= MAX_TRACKED && !failures.contains_key(user) {
            failures.clear();
        }
        let entry = failures.entry(user.to_owned()).or_insert((now, 0));
        if now.saturating_sub(entry.0) >= FAILURE_WINDOW_MILLIS {
            *entry = (now, 0);
        }
        entry.1 = entry.1.saturating_add(1);
    }

    /// The user `user` acts as when `password` is right.
    ///
    /// A password that passed is remembered, but a hit is only trusted while
    /// the stored user still exists with the hash it passed against, so a
    /// deleted user or a changed password stops working as soon as the
    /// snapshot moves. A wrong password is never remembered.
    ///
    /// # Errors
    ///
    /// [`Refusal`].
    pub fn authenticate(&self, user: &str, password: &str) -> Result<Arc<str>, Refusal> {
        let config = self.state.config().map_err(|_| Refusal::Unavailable)?;
        let now = (self.clock)();
        let Some(stored) = config.user(user) else {
            return Err(Refusal::Unauthorized);
        };
        if stored.hash.is_empty() {
            return Err(Refusal::Unauthorized);
        }
        let key = (user.to_owned(), self.keys.hash_one(password));
        {
            let mut passed = lock(&self.passed);
            match passed.get(&key) {
                Some(p) if p.hash == stored.hash => return Ok(Arc::from(user)),
                Some(_) => {
                    passed.remove(&key);
                }
                None => {}
            }
        }
        if self.throttled(user, now) {
            return Err(Refusal::Throttled);
        }
        let slot = self.slot().ok_or(Refusal::Throttled)?;
        let right = bcrypt::verify(password, &stored.hash).unwrap_or(false);
        drop(slot);
        if !right {
            self.failed(user, now);
            return Err(Refusal::Unauthorized);
        }
        let mut passed = lock(&self.passed);
        if passed.len() >= self.capacity {
            passed.clear();
        }
        passed.insert(
            key,
            Passed {
                hash: stored.hash.clone(),
            },
        );
        Ok(Arc::from(user))
    }

    /// Admits `header`'s user or says why not.
    ///
    /// # Errors
    ///
    /// [`Refusal`].
    pub fn admit(&self, header: Option<&str>) -> Result<Arc<str>, Refusal> {
        let (user, password) = header.and_then(basic).ok_or(Refusal::Unauthorized)?;
        self.authenticate(&user, &password)
    }
}

/// The guard that puts an [`Authenticator`] in front of every route.
pub struct AuthGuard(pub Arc<Authenticator>);

impl Guard for AuthGuard {
    fn admit(&self, req: &Admission<'_>) -> Verdict {
        match self.0.admit(req.authorization()) {
            Ok(user) => Verdict::Allow(Some(user)),
            Err(Refusal::Unauthorized) => Verdict::Deny(Denial {
                status: 401,
                content_type: TEXT,
                body: "Unauthorized".to_owned(),
                challenge: Some("Basic realm=\"OpenSearch Security\""),
            }),
            Err(Refusal::Throttled) => Verdict::Deny(Denial {
                status: 429,
                content_type: TEXT,
                body: "Too many authentication attempts".to_owned(),
                challenge: None,
            }),
            Err(Refusal::Unavailable) => Verdict::Deny(Denial {
                status: 503,
                content_type: TEXT,
                body: "The security configuration is unavailable".to_owned(),
                challenge: None,
            }),
        }
    }
}
