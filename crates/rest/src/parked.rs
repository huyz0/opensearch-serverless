//! The search queue: connections whose request waits for its tenant's turn,
//! off the worker pool (ADR 0056).
//!
//! An entry is the stream and the bytes read so far (the head and anything
//! read past it, at most one head buffer). It resumes by replay: a worker
//! serves it as a new connection whose input starts with those bytes,
//! holding the permit the offer reserved. No thread watches the queue:
//! workers that end a search offer the next entry, and idle workers sweep
//! for expired entries and closed peers.

use std::collections::{HashMap, VecDeque};
use std::io::{ErrorKind, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::admission::{Permit, QueueEvent, SearchAdmission};

/// One parked connection.
pub(crate) struct Entry {
    pub(crate) stream: TcpStream,
    pub(crate) bytes: Vec<u8>,
    tenant: Arc<str>,
    since: Instant,
}

impl Entry {
    pub(crate) fn new(stream: TcpStream, bytes: Vec<u8>, tenant: Arc<str>) -> Self {
        Self {
            stream,
            bytes,
            tenant,
            since: Instant::now(),
        }
    }
}

/// What the queue has done, for operators and tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QueueStats {
    /// Entries waiting now.
    pub parked: usize,
    /// Requests parked since the start.
    pub deferred: u64,
    /// Entries a worker took back to run.
    pub resumed: u64,
    /// Searches answered 429 because the queue was full.
    pub refused: u64,
    /// Entries answered 429 at the wait.
    pub expired: u64,
    /// Entries dropped because the peer closed.
    pub dropped: u64,
    /// The longest an entry waited, in milliseconds.
    pub longest_wait_millis: u64,
}

#[derive(Default)]
struct Inner {
    by_tenant: HashMap<Arc<str>, VecDeque<Entry>>,
    /// The tenants with entries, in rotation.
    ring: VecDeque<Arc<str>>,
    parked: usize,
    resumed: VecDeque<(Entry, Permit)>,
}

pub(crate) struct SearchQueue {
    admission: Arc<dyn SearchAdmission>,
    size: usize,
    wait: Duration,
    poll: Duration,
    inner: Mutex<Inner>,
    last_sweep: Mutex<Instant>,
    /// Resumed entries not yet taken by a worker.
    waiting: AtomicUsize,
    deferred: AtomicU64,
    resumed: AtomicU64,
    refused: AtomicU64,
    expired: AtomicU64,
    dropped: AtomicU64,
    longest: AtomicU64,
}

/// The answer to a search the queue refuses or lets expire.
fn too_many() -> String {
    // ALLOC: only when a search is refused; under 400 bytes.
    let body = r#"{"error":{"root_cause":[{"type":"es_rejected_execution_exception","reason":"rejected execution of search: the search queue is full or the wait ended"}],"type":"es_rejected_execution_exception","reason":"rejected execution of search: the search queue is full or the wait ended"},"status":429}"#;
    format!(
        "HTTP/1.1 429 Too Many Requests\r\ncontent-type: application/json; charset=UTF-8\r\nretry-after: 1\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
}

const UNAVAILABLE: &[u8] =
    b"HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";

/// How long a sweeper may block writing a small answer to a stuck peer.
const ANSWER_TIMEOUT: Duration = Duration::from_millis(500);

fn answer(stream: &TcpStream, bytes: &[u8]) {
    let _ = stream.set_write_timeout(Some(ANSWER_TIMEOUT));
    let mut s = stream;
    let _ = s.write_all(bytes);
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

/// Whether the peer has closed: a non-blocking peek reads end of stream or
/// an error. Bytes pending are not a close.
fn peer_closed(stream: &TcpStream) -> bool {
    if stream.set_nonblocking(true).is_err() {
        return true;
    }
    let mut one = [0u8; 1];
    let seen = stream.peek(&mut one);
    let _ = stream.set_nonblocking(false);
    match seen {
        Ok(0) => true,
        Ok(_) => false,
        Err(e) => !matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted),
    }
}

/// Whether a sweep is due: a full idle poll has passed since the last.
fn sweep_due(since_last: Duration, poll: Duration) -> bool {
    since_last >= poll
}

impl SearchQueue {
    pub(crate) fn new(
        admission: Arc<dyn SearchAdmission>,
        size: usize,
        wait: Duration,
        poll: Duration,
    ) -> Self {
        Self {
            admission,
            size,
            wait,
            poll,
            inner: Mutex::new(Inner::default()),
            last_sweep: Mutex::new(Instant::now()),
            waiting: AtomicUsize::new(0),
            deferred: AtomicU64::new(0),
            resumed: AtomicU64::new(0),
            refused: AtomicU64::new(0),
            expired: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            longest: AtomicU64::new(0),
        }
    }

    pub(crate) fn admission(&self) -> &dyn SearchAdmission {
        &*self.admission
    }

    /// Whether resumed entries wait for a worker, so an idle keep-alive
    /// connection should yield.
    pub(crate) fn has_waiting(&self) -> bool {
        self.waiting.load(Ordering::Acquire) > 0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn stats(&self) -> QueueStats {
        QueueStats {
            parked: self.lock().parked,
            deferred: self.deferred.load(Ordering::Relaxed),
            resumed: self.resumed.load(Ordering::Relaxed),
            refused: self.refused.load(Ordering::Relaxed),
            expired: self.expired.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
            longest_wait_millis: self.longest.load(Ordering::Relaxed),
        }
    }

    /// Parks `entry`; when the queue is full the entry is answered 429 and
    /// closed, and its connection counted closed in `open`.
    pub(crate) fn park(&self, entry: Entry, open: &AtomicUsize) {
        let mut guard = self.lock();
        let q = &mut *guard;
        if q.parked >= self.size {
            drop(guard);
            self.refused.fetch_add(1, Ordering::Relaxed);
            self.admission.note(QueueEvent::Refused);
            answer(&entry.stream, too_many().as_bytes());
            open.fetch_sub(1, Ordering::AcqRel);
            return;
        }
        self.deferred.fetch_add(1, Ordering::Relaxed);
        self.admission.note(QueueEvent::Deferred);
        let tenant = Arc::clone(&entry.tenant);
        let deque = q.by_tenant.entry(Arc::clone(&tenant)).or_default();
        let first = deque.is_empty();
        deque.push_back(entry);
        if first {
            q.ring.push_back(tenant);
        }
        q.parked = q.parked.saturating_add(1);
    }

    /// Moves every entry whose tenant may run now to the resumed list, each
    /// holding the permit its offer reserved: one pass over the ring, again
    /// while a pass found one.
    pub(crate) fn offer(&self) {
        let mut guard = self.lock();
        let q = &mut *guard;
        loop {
            let mut found = false;
            for _ in 0..q.ring.len() {
                let Some(tenant) = q.ring.pop_front() else {
                    break;
                };
                let Some(permit) = self.admission.try_reserve(&tenant) else {
                    q.ring.push_back(tenant);
                    continue;
                };
                let Some(deque) = q.by_tenant.get_mut(&tenant) else {
                    continue;
                };
                let Some(entry) = deque.pop_front() else {
                    q.by_tenant.remove(&tenant);
                    continue;
                };
                if deque.is_empty() {
                    q.by_tenant.remove(&tenant);
                } else {
                    q.ring.push_back(Arc::clone(&tenant));
                }
                q.parked = q.parked.saturating_sub(1);
                self.waiting.fetch_add(1, Ordering::AcqRel);
                q.resumed.push_back((entry, permit));
                found = true;
            }
            if !found {
                return;
            }
        }
    }

    /// The next resumed entry and the permit reserved for it.
    pub(crate) fn take_resumed(&self) -> Option<(Entry, Permit)> {
        if !self.has_waiting() {
            return None;
        }
        let taken = self.lock().resumed.pop_front()?;
        self.waiting.fetch_sub(1, Ordering::AcqRel);
        self.resumed.fetch_add(1, Ordering::Relaxed);
        let waited = u64::try_from(taken.0.since.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.longest.fetch_max(waited, Ordering::Relaxed);
        self.admission.note(QueueEvent::Resumed(waited));
        Some(taken)
    }

    /// Offers what may run, then answers entries past the wait and drops
    /// entries whose peer closed; at most once every idle poll, by one
    /// worker at a time.
    pub(crate) fn sweep(&self, open: &AtomicUsize) {
        {
            let Ok(mut last) = self.last_sweep.try_lock() else {
                return;
            };
            if !sweep_due(last.elapsed(), self.poll) {
                return;
            }
            *last = Instant::now();
        }
        self.offer();
        // ALLOC: the entries this sweep removes, at most the queue's size.
        let mut gone: Vec<(Entry, bool)> = Vec::new();
        {
            let mut guard = self.lock();
            let q = &mut *guard;
            // ALLOC: the tenants this sweep empties, at most the queue's size.
            let mut emptied = Vec::new();
            for (tenant, deque) in &mut q.by_tenant {
                let mut i = 0;
                while let Some(e) = deque.get(i) {
                    let verdict = if e.since.elapsed() >= self.wait {
                        Some(true)
                    } else if peer_closed(&e.stream) {
                        Some(false)
                    } else {
                        None
                    };
                    match verdict {
                        Some(expired) => {
                            if let Some(entry) = deque.remove(i) {
                                gone.push((entry, expired));
                            }
                        }
                        None => i = i.saturating_add(1),
                    }
                }
                if deque.is_empty() {
                    emptied.push(Arc::clone(tenant));
                }
            }
            for t in &emptied {
                q.by_tenant.remove(t);
                q.ring.retain(|r| r != t);
            }
            q.parked = q.parked.saturating_sub(gone.len());
        }
        for (entry, expired) in gone {
            if expired {
                self.expired.fetch_add(1, Ordering::Relaxed);
                self.admission.note(QueueEvent::Expired);
                answer(&entry.stream, too_many().as_bytes());
            } else {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                self.admission.note(QueueEvent::Dropped);
            }
            open.fetch_sub(1, Ordering::AcqRel);
        }
    }

    /// Answers every entry 503 and closes it, at shutdown.
    pub(crate) fn drain(&self, open: &AtomicUsize) {
        let (parked, resumed) = {
            let mut guard = self.lock();
            let q = &mut *guard;
            q.ring.clear();
            q.parked = 0;
            // ALLOC: every entry, at shutdown only.
            let parked: Vec<Entry> = q.by_tenant.drain().flat_map(|(_, d)| d).collect();
            // ALLOC: every resumed entry, at shutdown only.
            let resumed: Vec<Entry> = q.resumed.drain(..).map(|(e, _)| e).collect();
            (parked, resumed)
        };
        self.waiting.store(0, Ordering::Release);
        for entry in parked.into_iter().chain(resumed) {
            answer(&entry.stream, UNAVAILABLE);
            open.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sweep_is_due_once_a_full_poll_has_passed() {
        let poll = Duration::from_millis(50);
        assert!(!sweep_due(Duration::from_millis(49), poll));
        assert!(sweep_due(poll, poll));
        assert!(sweep_due(Duration::from_millis(51), poll));
    }
}
