//! Node-local jobs: a search that outlives the call that asked for it, kept in
//! memory with no blob request and no timer
//! ([ADR 0084](../../../../docs/adr/0084-asynchronous-search-is-a-native-plugin-over-node-local-jobs.md),
//! docs/specs/async-search).
//!
//! A job is data the host runs: the work is a closure the host builds from a
//! search request, never code a plugin supplies. Every limit is checked when
//! an entry is added or read; nothing wakes on its own.

mod types;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use oss_mem::{MemoryBudget, Pool, Reservation};

pub use types::{
    Clock, Ctx, ENTRY_OVERHEAD, Failure, JobError, Limits, State, Stats, View, Who, Work,
};

/// How a finished entry ended.
enum Done {
    Ok(Vec<u8>),
    Err(&'static str, String),
}

struct Entry {
    num: u64,
    who: Who,
    start: u64,
    expires: u64,
    cancel: Arc<AtomicBool>,
    done: Option<Done>,
    /// Finished at (milliseconds), for oldest-first eviction.
    end: u64,
    /// The reservation of a finished entry: its result and overhead.
    held: Option<Reservation>,
}

#[derive(Default)]
struct Inner {
    entries: Vec<Entry>,
    threads: usize,
    by_tenant: HashMap<String, usize>,
    stats: Stats,
}

/// The node's jobs.
pub struct Jobs {
    node: String,
    next: AtomicU64,
    budget: MemoryBudget,
    limits: Limits,
    clock: Clock,
    inner: Mutex<Inner>,
    changed: Condvar,
}

/// Whose a job id is.
#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    /// A job of this node, by number.
    Local(u64),
    /// A job of node `node`.
    Remote {
        /// The node's id.
        node: String,
        /// The job's number there.
        number: u64,
    },
}

/// Gives a thread's slot and reservation back when it ends, however it ends.
struct Slot {
    jobs: Arc<Jobs>,
    tenant: String,
    held: Option<Reservation>,
}

impl Drop for Slot {
    fn drop(&mut self) {
        drop(self.held.take());
        let mut inner = self.jobs.lock();
        inner.threads = inner.threads.saturating_sub(1);
        if let Some(n) = inner.by_tenant.get_mut(&self.tenant) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                inner.by_tenant.remove(&self.tenant);
            }
        }
        drop(inner);
        self.jobs.changed.notify_all();
    }
}

impl Jobs {
    /// An empty registry for node `node`; ids count up from the clock's first
    /// reading, so one never names a job of an earlier process.
    pub fn new(node: &str, budget: &MemoryBudget, limits: Limits, clock: Clock) -> Arc<Self> {
        let first = clock();
        Arc::new(Self {
            node: node.to_owned(), // ALLOC: one node id, once.
            next: AtomicU64::new(first),
            budget: budget.clone(),
            limits,
            clock,
            inner: Mutex::new(Inner::default()),
            changed: Condvar::new(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn now(&self) -> u64 {
        (self.clock)()
    }

    /// The limits in force.
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// Whose job `text` names.
    ///
    /// # Errors
    ///
    /// 400 for an id that is not `<node>:<number>`.
    pub fn route(&self, text: &str) -> Result<Target, JobError> {
        let bad = || JobError::BadRequest(format!("malformed job id [{text}]"));
        let (node, number) = text.rsplit_once(':').ok_or_else(bad)?;
        let number = number.parse::<u64>().map_err(|_| bad())?;
        if node.is_empty() {
            return Err(bad());
        }
        if node == self.node {
            Ok(Target::Local(number))
        } else {
            // ALLOC: one node id of a request, bounded by the path limit.
            Ok(Target::Remote {
                node: node.to_owned(),
                number,
            })
        }
    }

    /// Starts `work` on its own thread and returns the job's number.
    ///
    /// # Errors
    ///
    /// 400 for a `keep_alive` over the maximum; 429 at a limit, when the
    /// tenant's kept entries are all running, or when the pool cannot hold
    /// the entry.
    pub fn submit(
        self: &Arc<Self>,
        who: &Who,
        work: Work,
        keep_alive_ms: Option<u64>,
    ) -> Result<u64, JobError> {
        let keep = keep_alive_ms.unwrap_or(self.limits.default_keep_alive_ms());
        if keep > self.limits.max_keep_alive_ms {
            return Err(JobError::BadRequest(format!(
                "Keep alive for asynchronous search ({keep}ms) is too large. It must be less than ({}ms)",
                self.limits.max_keep_alive_ms
            )));
        }
        let now = self.now();
        let mut inner = self.lock();
        self.sweep(&mut inner, now);
        let mine = inner.by_tenant.get(&who.tenant).copied().unwrap_or(0);
        if inner.threads >= self.limits.max_running || mine >= self.limits.max_running_per_tenant {
            inner.stats.rejected = inner.stats.rejected.saturating_add(1);
            return Err(JobError::Rejected(
                "too many asynchronous searches are running",
            ));
        }
        let tenant_entries = inner.entries.iter().filter(|e| e.who.tenant == who.tenant);
        let (total, finished) = tenant_entries.fold((0usize, 0usize), |(t, f), e| {
            (
                t.saturating_add(1),
                f.saturating_add(usize::from(e.done.is_some())),
            )
        });
        if total >= self.limits.max_kept / 2 && finished == 0 {
            inner.stats.rejected = inner.stats.rejected.saturating_add(1);
            return Err(JobError::Rejected(
                "this tenant already holds the most asynchronous searches",
            ));
        }
        let held = self
            .budget
            .try_reserve(
                Pool::Extensions,
                self.limits
                    .max_response_bytes
                    .saturating_add(ENTRY_OVERHEAD),
            )
            .map_err(|_| {
                inner.stats.rejected = inner.stats.rejected.saturating_add(1);
                JobError::Rejected("this node's memory cannot hold another asynchronous search")
            })?;
        let num = self.next.fetch_add(1, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        let ctx = Ctx {
            cancel: Arc::clone(&cancel),
            deadline: now.saturating_add(self.limits.max_running_time_ms),
            clock: Arc::clone(&self.clock),
            max_bytes: self.limits.max_response_bytes,
        };
        // ALLOC: one entry a job, at most max_kept (64) held.
        inner.entries.push(Entry {
            num,
            who: who.clone(), // ALLOC: two short strings, once a job.
            start: now,
            expires: now.saturating_add(keep),
            cancel,
            done: None,
            end: now,
            held: None,
        });
        inner.threads = inner.threads.saturating_add(1);
        let n = inner.by_tenant.entry(who.tenant.clone()).or_insert(0);
        *n = n.saturating_add(1); // ALLOC: one key a tenant with a job.
        inner.stats.submitted = inner.stats.submitted.saturating_add(1);
        drop(inner);
        let slot = Slot {
            jobs: Arc::clone(self),
            tenant: who.tenant.clone(),
            held: Some(held),
        };
        let spawned = std::thread::Builder::new()
            .name("oss-job".into())
            .spawn(move || run(slot, num, ctx, work));
        if spawned.is_err() {
            // The closure was dropped with its slot, which gave the thread back.
            let mut inner = self.lock();
            inner.entries.retain(|e| e.num != num);
            inner.stats.rejected = inner.stats.rejected.saturating_add(1);
            return Err(JobError::Rejected("this node cannot start another thread"));
        }
        Ok(num)
    }

    /// The id text of job `num`.
    pub fn id(&self, num: u64) -> String {
        format!("{}:{num}", self.node)
    }

    /// Drops the finished entries past their expiry.
    fn sweep(&self, inner: &mut Inner, now: u64) {
        let before = inner.entries.len();
        inner
            .entries
            .retain(|e| e.done.is_none() || e.expires > now);
        let gone = before.saturating_sub(inner.entries.len());
        inner.stats.expired = inner.stats.expired.saturating_add(gone as u64);
    }

    /// Records how job `num` ended, and gives the entry its result's share of
    /// the pool in place of the running share.
    fn finish(
        &self,
        slot: &mut Slot,
        num: u64,
        result: std::thread::Result<Result<Vec<u8>, Failure>>,
    ) {
        let now = self.now();
        let mut inner = self.lock();
        let Some(at) = inner.entries.iter().position(|e| e.num == num) else {
            return; // deleted while it ran: the slot's drop frees its share.
        };
        let cancelled = inner
            .entries
            .get(at)
            .is_some_and(|e| e.cancel.load(Ordering::Relaxed));
        let done = match result {
            Ok(Ok(body)) => Done::Ok(body),
            Ok(Err(Failure::Stopped)) if cancelled => {
                Done::Err("task_cancelled", "cancelled".into())
            }
            Ok(Err(Failure::Stopped)) => Done::Err(
                "search_timeout",
                "the search ran past its time limit".into(),
            ),
            Ok(Err(Failure::TooLarge)) => Done::Err(
                "response_too_large",
                "the response is larger than the node keeps".into(),
            ),
            Ok(Err(Failure::Error(body))) => Done::Err("search_failed", body),
            Err(_) => Done::Err("panic", "the search panicked".into()),
        };
        let done = match done {
            Done::Ok(body) if body.len() > self.limits.max_response_bytes => Done::Err(
                "response_too_large",
                "the response is larger than the node keeps".into(),
            ),
            other => other,
        };
        let size = match &done {
            Done::Ok(body) => body.len(),
            Done::Err(_, text) => text.len().min(self.limits.max_response_bytes),
        };
        let mut held = slot.held.take();
        if let Some(r) = held.as_mut() {
            r.shrink(self.limits.max_response_bytes.saturating_sub(size));
        }
        if matches!(done, Done::Ok(_)) {
            inner.stats.search_completed = inner.stats.search_completed.saturating_add(1);
        } else {
            inner.stats.search_failed = inner.stats.search_failed.saturating_add(1);
        }
        if let Some(e) = inner.entries.get_mut(at) {
            e.done = Some(done);
            e.end = now;
            e.held = held;
        }
        self.evict(&mut inner);
        drop(inner);
        self.changed.notify_all();
    }

    /// Drops the oldest finished entries until the counts and bytes fit, a
    /// tenant's first when it holds more than half.
    fn evict(&self, inner: &mut Inner) {
        let half_n = self.limits.max_kept / 2;
        let half_b = self.limits.max_kept_bytes / 2;
        loop {
            let mut per: HashMap<&str, (usize, usize)> = HashMap::new();
            let mut b = 0usize;
            for e in inner.entries.iter().filter(|e| e.done.is_some()) {
                let bytes = e.held.as_ref().map_or(0, Reservation::bytes);
                let t = per.entry(e.who.tenant.as_str()).or_insert((0, 0));
                t.0 = t.0.saturating_add(1);
                t.1 = t.1.saturating_add(bytes);
                b = b.saturating_add(bytes);
            }
            let over_tenant = per
                .iter()
                .find(|(_, (c, by))| *c > half_n || *by > half_b)
                .map(|(t, _)| (*t).to_owned());
            let victim_tenant = over_tenant.or_else(|| {
                (inner.entries.len() > self.limits.max_kept || b > self.limits.max_kept_bytes)
                    .then(String::new)
            });
            let Some(tenant) = victim_tenant else { return };
            let oldest = inner
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.done.is_some() && (tenant.is_empty() || e.who.tenant == tenant))
                .min_by_key(|(_, e)| e.end)
                .map(|(i, _)| i);
            let Some(i) = oldest else { return };
            inner.entries.remove(i);
            inner.stats.evicted = inner.stats.evicted.saturating_add(1);
        }
    }

    /// Reads job `num` for `who`, waiting up to `wait_ms` for it to finish.
    ///
    /// # Errors
    ///
    /// 404 when there is no such job for this caller, or it has expired.
    pub fn get(&self, who: &Who, num: u64, wait_ms: u64) -> Result<View, JobError> {
        let until = Instant::now()
            .checked_add(Duration::from_millis(wait_ms))
            .unwrap_or_else(Instant::now);
        let mut inner = self.lock();
        loop {
            self.sweep(&mut inner, self.now());
            let e = inner
                .entries
                .iter()
                .find(|e| e.num == num && e.who == *who)
                .ok_or(JobError::NotFound)?;
            let left = until.saturating_duration_since(Instant::now());
            if e.done.is_some() || left.is_zero() {
                return Ok(self.view(e));
            }
            inner = self
                .changed
                .wait_timeout(inner, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    fn view(&self, e: &Entry) -> View {
        let (state, body, error) = match &e.done {
            None => (State::Running, None, None),
            Some(Done::Ok(b)) => (State::Succeeded, Some(b.clone()), None), // ALLOC: one result, at most max_response_bytes.
            Some(Done::Err(kind, text)) => (State::Failed, None, Some((*kind, text.clone()))), // ALLOC: one error text, bounded by the result cap.
        };
        View {
            id: self.id(e.num),
            state,
            start: e.start,
            expiration: e.expires,
            body,
            error,
        }
    }

    /// Sets the expiry of job `num` to `keep_alive_ms` from now.
    ///
    /// # Errors
    ///
    /// 400 over the maximum; 404 when there is no such job for this caller.
    pub fn extend(&self, who: &Who, num: u64, keep_alive_ms: u64) -> Result<(), JobError> {
        if keep_alive_ms > self.limits.max_keep_alive_ms {
            return Err(JobError::BadRequest(format!(
                "Keep alive for asynchronous search ({keep_alive_ms}ms) is too large. It must be less than ({}ms)",
                self.limits.max_keep_alive_ms
            )));
        }
        let now = self.now();
        let mut inner = self.lock();
        self.sweep(&mut inner, now);
        let e = inner
            .entries
            .iter_mut()
            .find(|e| e.num == num && e.who == *who)
            .ok_or(JobError::NotFound)?;
        e.expires = now.saturating_add(keep_alive_ms);
        Ok(())
    }

    /// Cancels a running job and drops the entry, or drops a finished one.
    /// A cancelled search's thread keeps its slot and its reservation until it
    /// exits.
    ///
    /// # Errors
    ///
    /// 404 when there is no such job for this caller.
    pub fn delete(&self, who: &Who, num: u64) -> Result<(), JobError> {
        let now = self.now();
        let mut inner = self.lock();
        self.sweep(&mut inner, now);
        let at = inner
            .entries
            .iter()
            .position(|e| e.num == num && e.who == *who)
            .ok_or(JobError::NotFound)?;
        let e = inner.entries.remove(at);
        if e.done.is_none() {
            e.cancel.store(true, Ordering::Relaxed);
            inner.stats.cancelled = inner.stats.cancelled.saturating_add(1);
        }
        drop(inner);
        self.changed.notify_all();
        Ok(())
    }

    /// This node's counts.
    pub fn stats(&self) -> Stats {
        let mut inner = self.lock();
        self.sweep(&mut inner, self.now());
        let mut s = inner.stats;
        s.running_current = inner.threads as u64;
        s.held_bytes = self.budget.in_use(Pool::Extensions) as u64;
        s
    }
}

/// The body of a job's thread.
fn run(mut slot: Slot, num: u64, ctx: Ctx, work: Work) {
    let jobs = Arc::clone(&slot.jobs);
    let result = catch_unwind(AssertUnwindSafe(|| work(&ctx)));
    jobs.finish(&mut slot, num, result);
}
