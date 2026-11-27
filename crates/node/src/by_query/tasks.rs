//! The tasks of by-query and reindex requests run in the background: node
//! memory only, bounded, with no timer and no blob request.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use oss_doc::mapping::json::write_str;
use oss_mem::{MemoryBudget, Pool, Reservation};
use oss_rest::{Exception, RestError};
use oss_transport::actions::tasks::{TaskRequest, TaskResponse};

use super::task_forward::TaskSend;
use crate::forward::ForwardError;

/// The most tasks running at once; each has a thread.
pub(super) const MAX_RUNNING: usize = 32;
/// The most finished tasks kept.
pub(super) const MAX_KEPT: usize = 128;
/// What each task holds from the budget, and the most one stored entry is.
pub(super) const ENTRY_BYTES: usize = 64 << 10;
/// How long a finished task is kept, in milliseconds.
pub(super) const TTL_MILLIS: u64 = 3_600_000;
/// The most bytes of a description or an error message kept.
const TEXT_MAX: usize = 1 << 10;
/// The most bytes of a stored response before its failures are dropped.
const RESPONSE_MAX: usize = 48 << 10;

/// How a finished task ended.
enum Outcome {
    /// The answer the synchronous call would have sent.
    Response(String),
    /// The error body.
    Error(String),
}

struct Entry {
    id: u64,
    action: &'static str,
    description: String,
    start: u64,
    end: u64,
    cancel: Arc<AtomicBool>,
    outcome: Option<Outcome>,
    _held: Reservation,
}

#[derive(Default)]
struct State {
    running: Vec<Entry>,
    kept: VecDeque<Entry>,
}

/// The node's tasks.
pub struct Tasks {
    node: String,
    next: AtomicU64,
    budget: MemoryBudget,
    state: Mutex<State>,
    forward: OnceLock<Arc<dyn TaskSend>>,
}

/// Whose a task id is.
pub(super) enum Target {
    /// A task of this node, by number.
    Local(u64),
    /// A task of node `node`.
    Remote {
        /// The node's id.
        node: String,
        /// The task's number there.
        number: u64,
    },
}

fn rejected(reason: &str) -> RestError {
    RestError::exception(
        429,
        Exception::new("es_rejected_execution_exception", reason.to_owned()), // ALLOC: error path.
    )
}

fn truncate(text: &str) -> String {
    let mut end = text.len().min(TEXT_MAX);
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    text[..end].to_owned() // ALLOC: at most TEXT_MAX bytes.
}

impl Tasks {
    /// An empty registry for node `node`; ids count up from `now_millis`, so
    /// one never names a task of an earlier process. Each task holds
    /// [`ENTRY_BYTES`] of `budget`'s request-body pool.
    pub fn new(node: &str, budget: &MemoryBudget, now_millis: u64) -> Self {
        Self {
            node: node.to_owned(), // ALLOC: one node id, once.
            next: AtomicU64::new(now_millis),
            budget: budget.clone(),
            state: Mutex::new(State::default()),
            forward: OnceLock::new(),
        }
    }

    /// Lets a task id of another node be answered by that node, once.
    pub fn set_forward(&self, forward: Arc<dyn TaskSend>) {
        self.forward.set(forward).ok();
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The node's id, as a task id names it.
    pub(super) fn node(&self) -> &str {
        &self.node
    }

    /// Drops the kept tasks older than the TTL.
    fn expire(state: &mut State, now: u64) {
        state
            .kept
            .retain(|e| now.saturating_sub(e.end) < TTL_MILLIS);
    }

    /// Registers a running task and returns its number and cancel flag.
    ///
    /// # Errors
    ///
    /// 429 at [`MAX_RUNNING`] or when the budget cannot hold the entry.
    pub(super) fn start(
        &self,
        action: &'static str,
        description: &str,
        now: u64,
    ) -> Result<(u64, Arc<AtomicBool>), RestError> {
        let mut state = self.lock();
        Self::expire(&mut state, now);
        if state.running.len() >= MAX_RUNNING {
            return Err(rejected("too many tasks are running on this node"));
        }
        let held = self
            .budget
            .try_reserve(Pool::RequestBodies, ENTRY_BYTES)
            .map_err(|_| rejected("this node's memory cannot hold another task"))?;
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        // ALLOC: one entry a task, at most MAX_RUNNING running.
        state.running.push(Entry {
            id,
            action,
            description: truncate(description),
            start: now,
            end: now,
            cancel: Arc::clone(&cancel),
            outcome: None,
            _held: held,
        });
        Ok((id, cancel))
    }

    /// Records how task `id` ended and moves it to the kept tasks.
    pub(super) fn finish(&self, id: u64, result: Result<String, RestError>, now: u64) {
        let outcome = match result {
            Ok(body) => Outcome::Response(fit_response(body)),
            Err(error) => {
                let mut out = Vec::new();
                let _ = error.write_body(&mut out);
                let body = String::from_utf8(out).unwrap_or_default();
                if body.len() > TEXT_MAX * 16 {
                    Outcome::Error(String::from(
                        r#"{"error":{"type":"exception","reason":"the error is too long to keep"},"status":500}"#,
                    ))
                } else {
                    Outcome::Error(body)
                }
            }
        };
        let mut state = self.lock();
        let Some(at) = state.running.iter().position(|e| e.id == id) else {
            return;
        };
        let mut entry = state.running.remove(at);
        entry.end = now;
        entry.outcome = Some(outcome);
        state.kept.push_back(entry);
        while state.kept.len() > MAX_KEPT {
            state.kept.pop_front();
        }
        Self::expire(&mut state, now);
    }

    /// Whose task `text` names.
    ///
    /// # Errors
    ///
    /// 400 for an id that is not `<node>:<number>`.
    pub(super) fn route(&self, text: &str) -> Result<Target, RestError> {
        let bad = || {
            RestError::illegal_argument(
                // ALLOC: error path; one message.
                format!("malformed task id {text}"),
            )
        };
        let (node, number) = text.rsplit_once(':').ok_or_else(bad)?;
        let number: u64 = number.parse().map_err(|_| bad())?;
        Ok(if node == self.node {
            Target::Local(number)
        } else {
            // ALLOC: one node id, on a forwarded request.
            Target::Remote {
                node: node.to_owned(),
                number,
            }
        })
    }

    /// This node's answer to `request`: what `GET /_tasks/{id}` or the
    /// cancel would send.
    pub fn call(&self, request: TaskRequest, now: u64) -> TaskResponse {
        let result = if request.cancel {
            self.cancel(request.number, now)
        } else {
            self.get(request.number, now)
        };
        match result {
            Ok(body) => TaskResponse { status: 200, body },
            Err(error) => {
                let mut out = Vec::new();
                let _ = error.write_body(&mut out);
                TaskResponse {
                    status: error.status(),
                    body: String::from_utf8(out).unwrap_or_default(),
                }
            }
        }
    }

    /// Asks node `node` to answer `request`, for a task id that names it.
    ///
    /// # Errors
    ///
    /// 404 when no forwarding is wired or no live node holds the id's node,
    /// 503 when the call failed.
    pub(super) fn remote(
        &self,
        node: &str,
        request: TaskRequest,
    ) -> Result<TaskResponse, RestError> {
        let gone = || {
            // ALLOC: error path; one message.
            not_found(format!(
                "task [{node}:{}] belongs to node [{node}], which this node cannot reach",
                request.number
            ))
        };
        let Some(forward) = self.forward.get() else {
            return Err(gone());
        };
        match forward.call(node, request) {
            Ok(answer) => Ok(answer),
            Err(ForwardError::NoPeer | ForwardError::Gone) => Err(gone()),
            Err(error) => Err(RestError::simple(
                503,
                // ALLOC: error path; one message.
                format!("the node holding the task did not answer: {error}"),
            )),
        }
    }

    /// `GET /_tasks/{id}`'s body.
    ///
    /// # Errors
    ///
    /// 404 for a task that is not here (never ran, restarted away, expired).
    pub(super) fn get(&self, id: u64, now: u64) -> Result<String, RestError> {
        let mut state = self.lock();
        Self::expire(&mut state, now);
        let entry = state
            .running
            .iter()
            .chain(state.kept.iter())
            .find(|e| e.id == id)
            .ok_or_else(|| not_found(format!("task [{}:{id}] isn't found", self.node)))?;
        let mut out = String::new();
        let _ = write!(
            out,
            r#"{{"completed":{},"task":{}"#,
            entry.outcome.is_some(),
            self.task_json(entry, now)
        );
        match &entry.outcome {
            Some(Outcome::Response(body)) => {
                let _ = write!(out, r#","response":{body}"#);
            }
            Some(Outcome::Error(body)) => {
                // The error body is `{"error":{..},"status":n}`: its error member.
                let inner = body
                    .strip_prefix(r#"{"error":"#)
                    .and_then(|b| b.rsplit_once(r#","status":"#))
                    .map_or("null", |(error, _)| error);
                let _ = write!(out, r#","error":{inner}"#);
            }
            None => {}
        }
        out.push('}');
        Ok(out)
    }

    /// `GET /_tasks`'s body: the running tasks, those of `actions` when given.
    pub(super) fn running_json(&self, actions: Option<&str>, now: u64) -> String {
        let mut state = self.lock();
        Self::expire(&mut state, now);
        let mut out = String::new();
        out.push_str(r#"{"nodes":{"#);
        write_str(&mut out, &self.node);
        out.push_str(r#":{"name":"#);
        write_str(&mut out, &self.node);
        out.push_str(r#","tasks":{"#);
        let mut first = true;
        for entry in &state.running {
            if !matches_actions(actions, entry.action) {
                continue;
            }
            if !first {
                out.push(',');
            }
            first = false;
            let _ = write!(
                out,
                r#""{}:{}":{}"#,
                self.node,
                entry.id,
                self.task_json(entry, now)
            );
        }
        out.push_str("}}}}");
        out
    }

    /// Cancels running task `id` and answers `POST /_tasks/{id}/_cancel`.
    ///
    /// # Errors
    ///
    /// 404 for a task that is not running.
    pub(super) fn cancel(&self, id: u64, now: u64) -> Result<String, RestError> {
        let mut state = self.lock();
        Self::expire(&mut state, now);
        let entry = state
            .running
            .iter()
            .find(|e| e.id == id)
            .ok_or_else(|| not_found(format!("task [{}:{id}] isn't running", self.node)))?;
        entry.cancel.store(true, Ordering::Relaxed);
        let mut out = String::from(r#"{"nodes":{"#);
        write_str(&mut out, &self.node);
        out.push_str(r#":{"name":"#);
        write_str(&mut out, &self.node);
        let _ = write!(
            out,
            r#","tasks":{{"{}:{id}":{}}}}}}}}}"#,
            self.node,
            self.task_json(entry, now)
        );
        Ok(out)
    }

    fn task_json(&self, entry: &Entry, now: u64) -> String {
        let end = if entry.outcome.is_some() {
            entry.end
        } else {
            now
        };
        let nanos = end.saturating_sub(entry.start).saturating_mul(1_000_000);
        let mut out = String::new();
        out.push_str(r#"{"node":"#);
        write_str(&mut out, &self.node);
        let _ = write!(
            out,
            r#","id":{},"type":"transport","action":"{}","description":"#,
            entry.id, entry.action
        );
        write_str(&mut out, &entry.description);
        let _ = write!(
            out,
            r#","start_time_in_millis":{},"running_time_in_nanos":{nanos},"cancellable":true,"cancelled":{},"headers":{{}}}}"#,
            entry.start,
            entry.cancel.load(Ordering::Relaxed)
        );
        out
    }
}

fn matches_actions(filter: Option<&str>, action: &str) -> bool {
    filter.is_none_or(|list| {
        list.split(',')
            .any(|pattern| match pattern.strip_suffix('*') {
                Some(prefix) => action.starts_with(prefix),
                None => pattern == action,
            })
    })
}

fn not_found(reason: String) -> RestError {
    RestError::exception(404, Exception::new("resource_not_found_exception", reason))
}

/// `body` when it fits an entry, else with its failures dropped.
fn fit_response(body: String) -> String {
    if body.len() <= RESPONSE_MAX {
        return body;
    }
    match body.find(r#""failures":["#) {
        Some(at) => {
            let mut cut = body;
            cut.truncate(at);
            cut.push_str(r#""failures":[]}"#);
            cut
        }
        None => String::from("{}"),
    }
}

#[cfg(test)]
mod tests;

/// Records a task as failed when its thread ends without a result, even by
/// a panic, so its slot is never lost.
pub(super) struct Slot {
    tasks: Arc<Tasks>,
    id: u64,
    now: fn() -> u64,
    done: bool,
}

impl Slot {
    pub(super) fn new(tasks: Arc<Tasks>, id: u64, now: fn() -> u64) -> Self {
        Self {
            tasks,
            id,
            now,
            done: false,
        }
    }

    pub(super) fn finish(mut self, result: Result<String, RestError>) {
        self.done = true;
        self.tasks.finish(self.id, result, (self.now)());
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        if !self.done {
            self.tasks.finish(
                self.id,
                Err(RestError::simple(500, "the task ended without a result")),
                (self.now)(),
            );
        }
    }
}
