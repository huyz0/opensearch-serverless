//! Who a request is: HTTP basic credentials checked against the snapshot,
//! with a bounded bcrypt check ([ADR 0043](../../../docs/adr/0043-security-configuration-as-document-sets.md)).

use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use oss_coordinator::security::{admissible, basic};
use oss_rest::{Admission, Denial, Guard, Identity, Verdict};

use crate::security_jwt::Jwt;
use crate::security_state::{Clock, SecurityState};

/// The most password checks that run at once; the next is refused.
pub const MAX_VERIFICATIONS: usize = 4;
/// Checks remembered as passed.
pub const MAX_CACHED: usize = 4096;
/// Wrong passwords for one user within the window before it is throttled.
pub const MAX_FAILURES: u32 = 5;
/// The failure window.
pub const FAILURE_WINDOW_MILLIS: u64 = 10_000;
/// Names whose failures are remembered.
const MAX_TRACKED: usize = 4096;
/// The highest bcrypt cost a stored hash may ask for; a higher one is
/// treated as a wrong password without being run.
const MAX_COST: u32 = 15;

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
    /// A hash checked in place of a missing user's, so an unknown name costs
    /// what a known one does.
    decoy: String,
    running: AtomicUsize,
    slots: usize,
    capacity: usize,
    clock: Clock,
    jwt: Option<Jwt>,
    /// Failed tokens in the current window: one shared bucket, so forged
    /// subjects grow nothing.
    token_failures: Mutex<(u64, u32)>,
}

/// The cost a bcrypt hash asks for (`$2b$12$...`); past the cap when it is
/// not a hash at all.
fn cost_of(hash: &str) -> u32 {
    hash.split('$')
        .nth(2)
        .and_then(|c| c.parse().ok())
        .unwrap_or(u32::MAX)
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
            // The hash of a password nobody sends, at the default cost.
            decoy: bcrypt::hash("decoy", crate::security_route::DEFAULT_COST).unwrap_or_default(),
            running: AtomicUsize::new(0),
            slots: MAX_VERIFICATIONS,
            capacity: MAX_CACHED,
            clock,
            jwt: None,
            token_failures: Mutex::new((0, 0)),
        }
    }

    /// Whether a JSON Web Token domain is configured.
    pub fn has_jwt(&self) -> bool {
        self.jwt.is_some()
    }

    /// Accepts the tokens of `settings`' domain, if there is one.
    #[must_use]
    pub fn with_domain(self, settings: Option<&crate::security_setting::JwtSettings>) -> Self {
        match settings {
            None => self,
            Some(s) => self.with_jwt(Jwt::new(
                s.key.clone(),
                s.rules.clone(),
                Arc::new(crate::clock::wall_secs),
            )),
        }
    }

    /// Accepts tokens signed with the domain's key.
    #[must_use]
    pub fn with_jwt(mut self, jwt: Jwt) -> Self {
        self.jwt = Some(jwt);
        self
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
            failures.retain(|_, (since, _)| now.saturating_sub(*since) < FAILURE_WINDOW_MILLIS);
            if failures.len() >= MAX_TRACKED {
                // Every tracked name is inside its window: none is dropped,
                // so no throttle is lost to a flood of new names.
                return;
            }
        }
        // ALLOC: one name, at most MAX_TRACKED of them.
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
        let stored = config.user(user).filter(|u| !u.hash.is_empty());
        let key = (user.to_owned(), self.keys.hash_one(password));
        if let Some(stored) = stored {
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
        // A missing user is checked against the decoy: same cost, never right.
        let hash = stored.map_or(self.decoy.as_str(), |u| u.hash.as_str());
        let right = stored.is_some()
            && cost_of(hash) <= MAX_COST
            && bcrypt::verify(password, hash).unwrap_or(false);
        if stored.is_none() {
            let _ = bcrypt::verify(password, &self.decoy);
        }
        drop(slot);
        let Some(stored) = stored.filter(|_| right) else {
            self.failed(user, now);
            return Err(Refusal::Unauthorized);
        };
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

    /// Admits a bearer `token`, or says why not. A throttled bucket refuses
    /// before any signature is checked.
    fn admit_token(&self, token: &str) -> Result<Arc<Identity>, Refusal> {
        let jwt = self.jwt.as_ref().ok_or(Refusal::Unauthorized)?;
        let config = self.state.config().map_err(|_| Refusal::Unavailable)?;
        let now = (self.clock)();
        {
            let bucket = lock(&self.token_failures);
            if now.saturating_sub(bucket.0) < FAILURE_WINDOW_MILLIS && bucket.1 >= MAX_FAILURES {
                return Err(Refusal::Throttled);
            }
        }
        let slot = self.slot().ok_or(Refusal::Throttled)?;
        let verified = jwt.verify(token);
        drop(slot);
        match verified.filter(|c| admissible(&config, c)) {
            Some(claims) => Ok(Identity::token(Arc::from(claims.subject), claims.roles)),
            None => {
                let mut bucket = lock(&self.token_failures);
                if now.saturating_sub(bucket.0) >= FAILURE_WINDOW_MILLIS {
                    *bucket = (now, 0);
                }
                bucket.1 = bucket.1.saturating_add(1);
                Err(Refusal::Unauthorized)
            }
        }
    }

    /// Admits `header`'s user, basic or bearer, or says why not.
    ///
    /// # Errors
    ///
    /// [`Refusal`].
    pub fn admit_identity(&self, header: Option<&str>) -> Result<Arc<Identity>, Refusal> {
        let bearer = header
            .and_then(|h| h.split_once(' '))
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
            .map(|(_, token)| token.trim());
        match bearer {
            Some(token) => self.admit_token(token),
            None => self.admit(header).map(Identity::stored),
        }
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
        match self.0.admit_identity(req.authorization()) {
            Ok(identity) => Verdict::Allow(Some(identity)),
            Err(Refusal::Unauthorized) => Verdict::Deny(Denial {
                status: 401,
                content_type: TEXT,
                body: "Unauthorized".to_owned(),
                challenge: Some(if self.0.has_jwt() {
                    "Basic realm=\"OpenSearch Security\", Bearer"
                } else {
                    "Basic realm=\"OpenSearch Security\""
                }),
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
