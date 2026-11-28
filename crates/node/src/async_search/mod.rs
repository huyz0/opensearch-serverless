//! Asynchronous search: submit, get, delete and stats, as plain functions of
//! a request in and an answer out, over the node's job registry
//! ([ADR 0084](../../../../docs/adr/0084-asynchronous-search-is-a-native-plugin-over-node-local-jobs.md),
//! docs/specs/async-search).
//!
//! The search itself is a [`Runner`]'s: it checks what the search refuses
//! before a job starts and gives the work the job runs. Nothing here makes a
//! request to the store, starts a timer or holds a lock across a search.

mod params;
mod render;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use oss_rest::RestError;

use crate::jobs::{Jobs, Target, Who, Work};
use render::Shown;

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
        }
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
        let num = self.local(id)?;
        if let Some(ms) = p.keep_alive_ms {
            self.jobs
                .extend(who, num, ms)
                .map_err(|e| render::refusal(e, id))?;
        }
        let view = self
            .jobs
            .get(who, num, p.wait_ms)
            .map_err(|e| render::refusal(e, id))?;
        Ok(Reply {
            status: 200,
            body: render::envelope(&view, Shown::Resident),
        })
    }

    /// Cancels a running job or drops a kept one.
    ///
    /// # Errors
    ///
    /// 404 for an id that is not a job of this caller on this node.
    pub fn delete(&self, who: &Who, id: &str) -> Result<Reply, RestError> {
        let num = self.local(id)?;
        self.jobs
            .delete(who, num)
            .map_err(|e| render::refusal(e, id))?;
        Ok(Reply {
            status: 200,
            body: render::acknowledged(),
        })
    }

    /// This node's counts.
    pub fn stats(&self) -> Reply {
        Reply {
            status: 200,
            body: render::stats(&self.cluster, &self.node, &self.jobs.stats()),
        }
    }

    /// The number of a job of this node, or the refusal for any other id.
    fn local(&self, id: &str) -> Result<u64, RestError> {
        match self.jobs.route(id) {
            Ok(Target::Local(n)) => Ok(n),
            _ => Err(render::not_found(id)),
        }
    }
}
