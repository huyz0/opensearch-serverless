//! Asynchronous search: submit, get, delete and stats, as plain functions of
//! a request in and an answer out, over the node's job registry
//! ([ADR 0084](../../../../docs/adr/0084-asynchronous-search-is-a-native-plugin-over-node-local-jobs.md),
//! docs/specs/async-search).
//!
//! The search itself is a [`Runner`]'s: it checks what the search refuses
//! before a job starts and gives the work the job runs. Nothing here makes a
//! request to the store, starts a timer or holds a lock across a search.

#[cfg(all(feature = "reader", feature = "writer"))]
mod forward;
#[cfg(all(feature = "reader", feature = "writer"))]
mod mount;
mod params;
mod relay;
mod render;
#[cfg(all(feature = "reader", feature = "writer"))]
mod routes;
#[cfg(all(feature = "reader", feature = "writer"))]
mod runner;
#[cfg(test)]
mod tests;

use std::sync::{Arc, OnceLock};

use oss_rest::RestError;

use crate::jobs::{Jobs, Target, Who, Work};
#[cfg(all(feature = "reader", feature = "writer"))]
pub use forward::{AsyncSearchForwarder, AsyncSearchHandler};
#[cfg(all(feature = "reader", feature = "writer"))]
pub(crate) use mount::mount;
pub use relay::{Call, Op, Relay, RelayError};
use render::Shown;
#[cfg(all(feature = "reader", feature = "writer"))]
pub use routes::routes;
#[cfg(all(feature = "reader", feature = "writer"))]
pub use runner::SearchRunner;

/// One submit.
pub struct Submit<'a> {
    /// The index expression of the path, if it had one.
    pub index: Option<&'a str>,
    /// The request path, for the messages that name it.
    pub path: &'a str,
    /// The query parameters.
    pub query: &'a [(String, String)],
    /// The search body.
    pub body: &'a [u8],
    /// Who asked.
    pub who: &'a Who,
}

/// The search a submit starts.
pub struct Plan {
    /// What the job runs; its answer is the search response.
    pub work: Work,
    /// What a read of the running job shows: a partial search response.
    pub partial: String,
}

/// Plans searches.
pub trait Runner: Send + Sync {
    /// Checks the search and gives its work.
    ///
    /// # Errors
    ///
    /// What the search refuses before it runs (a body or query it cannot
    /// parse): answered at once, with no job.
    fn plan(&self, submit: &Submit<'_>, search: &[(String, String)]) -> Result<Plan, RestError>;
}

/// An answer: a status and a JSON body.
#[derive(Debug, PartialEq, Eq)]
pub struct Reply {
    /// The HTTP status.
    pub status: u16,
    /// The body.
    pub body: Vec<u8>,
}

/// The routes' logic.
pub struct Service {
    jobs: Arc<Jobs>,
    runner: Arc<dyn Runner>,
    cluster: String,
    node: String,
    relay: OnceLock<Arc<dyn Relay>>,
}

impl Service {
    /// A service over `jobs`, planning with `runner`, answering as node
    /// `node` of cluster `cluster`.
    pub fn new(jobs: Arc<Jobs>, runner: Arc<dyn Runner>, cluster: &str, node: &str) -> Self {
        Self {
            jobs,
            runner,
            cluster: cluster.to_owned(), // ALLOC: one name, once.
            node: node.to_owned(),       // ALLOC: one name, once.
            relay: OnceLock::new(),
        }
    }

    /// Sets how another node's ids are sent to it; once.
    pub fn set_relay(&self, relay: Arc<dyn Relay>) {
        let _ = self.relay.set(relay);
    }

    /// Starts a search and answers it as the stock plugin does: finished
    /// inside the wait, or still running with an id.
    ///
    /// # Errors
    ///
    /// 400 for a bad parameter or what the search refuses; 429 at a limit.
    pub fn submit(&self, submit: &Submit<'_>) -> Result<Reply, RestError> {
        let limits = self.jobs.limits();
        let p = params::submit(submit.query, submit.path, limits.max_keep_alive_ms)?;
        let plan = self.runner.plan(submit, &p.search)?;
        let (num, view) = self
            .jobs
            .submit(
                submit.who,
                plan.work,
                p.keep_alive_ms,
                p.keep,
                plan.partial,
                p.wait_ms,
            )
            .map_err(|e| render::refusal(e, ""))?;
        let _ = num;
        let shown = if p.keep {
            Shown::Persisting
        } else {
            Shown::Closed
        };
        Ok(Reply {
            status: 200,
            body: render::envelope(&view, shown),
        })
    }

    /// Reads a job, waiting for it when asked, and extends its keep-alive
    /// when asked.
    ///
    /// # Errors
    ///
    /// 404 for an id that is not a job of this caller on this node; 400 for a
    /// bad parameter.
    pub fn get(
        &self,
        who: &Who,
        id: &str,
        path: &str,
        query: &[(String, String)],
    ) -> Result<Reply, RestError> {
        let p = params::read(query, path, self.jobs.limits().max_keep_alive_ms)?;
        match self.target(id)? {
            Target::Local(num) => self.read_local(who, id, num, p.wait_ms, p.keep_alive_ms),
            Target::Remote { node, number } => self.send(
                &node,
                id,
                &Call {
                    op: Op::Get,
                    number,
                    who: who.clone(), // ALLOC: one caller per relayed call.
                    wait_ms: p.wait_ms,
                    keep_alive_ms: p.keep_alive_ms,
                },
            ),
        }
    }

    /// Cancels a running job or drops a kept one.
    ///
    /// # Errors
    ///
    /// 404 for an id that is not a job of this caller; 400 for a malformed
    /// id.
    pub fn delete(&self, who: &Who, id: &str) -> Result<Reply, RestError> {
        match self.target(id)? {
            Target::Local(num) => self.delete_local(who, id, num),
            Target::Remote { node, number } => self.send(
                &node,
                id,
                &Call {
                    op: Op::Delete,
                    number,
                    who: who.clone(), // ALLOC: one caller per relayed call.
                    wait_ms: 0,
                    keep_alive_ms: None,
                },
            ),
        }
    }

    /// Answers a call another node relayed: always from this node's own
    /// jobs, never relayed again, for the caller the first node verified.
    pub fn answer(&self, call: &Call) -> Reply {
        let id = self.jobs.id(call.number);
        let result = match call.op {
            Op::Get => self.read_local(
                &call.who,
                &id,
                call.number,
                call.wait_ms.min(self.jobs.limits().max_running_time_ms),
                call.keep_alive_ms,
            ),
            Op::Delete => self.delete_local(&call.who, &id, call.number),
        };
        result.unwrap_or_else(|e| {
            let mut body = Vec::new();
            let _ = e.write_body(&mut body);
            Reply {
                status: e.status(),
                body,
            }
        })
    }

    fn read_local(
        &self,
        who: &Who,
        id: &str,
        num: u64,
        wait_ms: u64,
        keep_alive_ms: Option<u64>,
    ) -> Result<Reply, RestError> {
        if let Some(ms) = keep_alive_ms {
            self.jobs
                .extend(who, num, ms)
                .map_err(|e| render::refusal(e, id))?;
        }
        let view = self
            .jobs
            .get(who, num, wait_ms)
            .map_err(|e| render::refusal(e, id))?;
        Ok(Reply {
            status: 200,
            body: render::envelope(&view, Shown::Resident),
        })
    }

    fn delete_local(&self, who: &Who, id: &str, num: u64) -> Result<Reply, RestError> {
        self.jobs
            .delete(who, num)
            .map_err(|e| render::refusal(e, id))?;
        Ok(Reply {
            status: 200,
            body: render::acknowledged(),
        })
    }

    /// Sends `call` to `node`: its answer as it gave it, or a 404 when no
    /// live node has that id or this node cannot reach one.
    fn send(&self, node: &str, id: &str, call: &Call) -> Result<Reply, RestError> {
        let Some(relay) = self.relay.get() else {
            return Err(render::unreachable(node, id));
        };
        match relay.call(node, call) {
            Ok((status, body)) => Ok(Reply { status, body }),
            Err(RelayError::NoNode) => Err(render::unreachable(node, id)),
            Err(RelayError::Failed) => Err(render::relay_failed(node)),
        }
    }

    /// This node's counts.
    pub fn stats(&self) -> Reply {
        Reply {
            status: 200,
            body: render::stats(&self.cluster, &self.node, &self.jobs.stats()),
        }
    }

    /// Whose an id is; one that is malformed is no job, so 404 as any id that
    /// is not this caller's.
    fn target(&self, id: &str) -> Result<Target, RestError> {
        self.jobs.route(id).map_err(|_| render::not_found(id))
    }
}
