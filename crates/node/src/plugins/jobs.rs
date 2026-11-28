//! The `job.*` host operations of the plugin system, over the node's job
//! registry: a plugin names a search and the node runs it on a thread of its
//! own ([ADR 0084](../../../../docs/adr/0084-asynchronous-search-is-a-native-plugin-over-node-local-jobs.md),
//! docs/specs/plugin-system Host API 1.0).
//!
//! The registry is the plugins' own, apart from the one the node's routes
//! use, and its results are reserved from the `Extensions` pool, which is the
//! pool a plugin's calls are charged to. A job belongs to the plugin that
//! started it, in the tenant the call served.

use std::sync::Arc;

use oss_ext::host::{HostError, Who};
use oss_ext::job::{JobSink, JobState, JobStats, JobView};

use crate::async_search::{Runner, Submit};
use crate::jobs::{JobError, Jobs, State, Target, Who as JobWho};

/// The longest a `job.get` waits for a job, in milliseconds.
const MAX_WAIT_MS: u64 = 10_000;

/// Runs the searches plugins ask for.
pub struct PluginJobs {
    jobs: Arc<Jobs>,
    runner: Arc<dyn Runner>,
}

impl PluginJobs {
    /// A sink starting jobs in `jobs`, planning each search with `runner`.
    #[must_use]
    pub fn new(jobs: Arc<Jobs>, runner: Arc<dyn Runner>) -> Self {
        Self { jobs, runner }
    }

    /// Whose a job is: the plugin, in the tenant of the call.
    fn owner(who: &Who<'_>) -> JobWho {
        JobWho {
            // ALLOC: two short strings a call.
            tenant: if who.tenant.is_empty() {
                "_".to_owned()
            } else {
                who.tenant.to_owned()
            },
            user: format!("plugin:{}", who.plugin),
        }
    }

    fn number(&self, id: &str) -> Result<u64, HostError> {
        match self.jobs.route(id) {
            Ok(Target::Local(n)) => Ok(n),
            // A job of another node, or not an id: not this plugin's job.
            _ => Err(HostError::Invalid),
        }
    }
}

fn refused(e: &JobError) -> HostError {
    match e {
        JobError::Rejected(_) => HostError::Budget,
        JobError::NotFound | JobError::BadRequest(_) => HostError::Invalid,
    }
}

impl JobSink for PluginJobs {
    fn submit(&self, who: &Who<'_>, index: &str, body: &[u8]) -> Result<String, HostError> {
        let owner = Self::owner(who);
        let plan = self
            .runner
            .plan(
                &Submit {
                    index: Some(index),
                    path: "",
                    query: &[],
                    body,
                    who: &owner,
                },
                &[],
            )
            .map_err(|_| HostError::Invalid)?;
        // Kept until the plugin deletes it or its keep-alive passes.
        let (num, _) = self
            .jobs
            .submit(&owner, plan.work, None, true, plan.partial, 0)
            .map_err(|e| refused(&e))?;
        Ok(self.jobs.id(num))
    }

    fn get(&self, who: &Who<'_>, id: &str, wait_ms: u64) -> Result<JobView, HostError> {
        let num = self.number(id)?;
        let view = self
            .jobs
            .get(&Self::owner(who), num, wait_ms.min(MAX_WAIT_MS))
            .map_err(|e| refused(&e))?;
        Ok(match view.state {
            State::Running => JobView {
                state: JobState::Running,
                body: Vec::new(),
            },
            State::Succeeded => JobView {
                state: JobState::Succeeded,
                body: view.body.unwrap_or_default(),
            },
            State::Failed => JobView {
                state: JobState::Failed,
                body: view
                    .error
                    .map(|(_, body)| body.into_bytes())
                    .unwrap_or_default(),
            },
        })
    }

    fn delete(&self, who: &Who<'_>, id: &str) -> Result<(), HostError> {
        let num = self.number(id)?;
        self.jobs
            .delete(&Self::owner(who), num)
            .map_err(|e| refused(&e))
    }

    fn stats(&self, _: &Who<'_>) -> Result<JobStats, HostError> {
        let s = self.jobs.stats();
        Ok(JobStats {
            running: s.running_current,
            submitted: s.submitted,
            cancelled: s.cancelled,
            rejected: s.rejected,
        })
    }
}
