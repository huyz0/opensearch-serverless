//! docs/specs/plugin-system Host API 1.0: the `job.*` operations over the
//! node's job registry.

use std::sync::{Arc, Condvar, Mutex};

use oss_ext::host::{HostError, Who};
use oss_ext::job::{JobSink, JobState};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_rest::RestError;

use super::PluginJobs;
use crate::async_search::{Plan, Runner, Submit};
use crate::jobs::{Clock, Failure, Jobs, Limits, Work};

#[derive(Default)]
struct Gate(Mutex<bool>, Condvar);

/// A search that answers `{}`, held at a gate when its body says `slow`,
/// failing when it says `error`, refused when it says `refuse`.
struct Fake(Arc<Gate>);

impl Runner for Fake {
    fn plan(&self, s: &Submit<'_>, _: &[(String, String)]) -> Result<Plan, RestError> {
        let body = String::from_utf8_lossy(s.body).into_owned();
        if body.contains("refuse") {
            return Err(RestError::illegal_argument("no"));
        }
        let gate = Arc::clone(&self.0);
        let work: Work = Box::new(move |_| {
            if body.contains("slow") {
                let mut open = gate.0.lock().expect("gate");
                while !*open {
                    open = gate.1.wait(open).expect("gate");
                }
            }
            if body.contains("error") {
                return Err(Failure::Error(r#"{"type":"x","reason":"y"}"#.into()));
            }
            Ok(b"{}".to_vec())
        });
        Ok(Plan {
            work,
            partial: String::new(),
        })
    }
}

fn who<'a>(plugin: &'a str, tenant: &'a str) -> Who<'a> {
    Who {
        plugin,
        tenant,
        grants: &[],
    }
}

fn rig(extensions: usize) -> (PluginJobs, Arc<Gate>) {
    let config = BudgetConfig::new(256 << 20)
        .with_pool(Pool::Extensions, extensions)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let clock: Clock = Arc::new(|| 1_000);
    let gate = Arc::new(Gate::default());
    let jobs = Jobs::new("n1", &budget, Limits::default(), clock);
    (
        PluginJobs::new(jobs, Arc::new(Fake(Arc::clone(&gate)))),
        gate,
    )
}

#[test]
fn a_plugin_starts_reads_and_deletes_its_own_job() {
    let (p, _) = rig(64 << 20);
    let id = p.submit(&who("a", "t"), "idx", b"{}").expect("starts");
    let got = p.get(&who("a", "t"), &id, 5_000).expect("reads");
    assert_eq!(
        (got.state, got.body.as_slice()),
        (JobState::Succeeded, &b"{}"[..])
    );
    // Kept until deleted.
    assert!(p.get(&who("a", "t"), &id, 0).is_ok());
    p.delete(&who("a", "t"), &id).expect("deleted");
    assert_eq!(p.get(&who("a", "t"), &id, 0), Err(HostError::Invalid));
    assert_eq!(p.stats(&who("a", "t")).expect("stats").submitted, 1);
}

#[test]
fn another_plugin_or_tenant_does_not_see_the_job() {
    let (p, _) = rig(64 << 20);
    let id = p.submit(&who("a", "t"), "idx", b"{}").expect("starts");
    for other in [who("b", "t"), who("a", "u")] {
        assert_eq!(p.get(&other, &id, 0), Err(HostError::Invalid));
        assert_eq!(p.delete(&other, &id), Err(HostError::Invalid));
    }
    assert_eq!(p.get(&who("a", "t"), "n2:1", 0), Err(HostError::Invalid));
    assert_eq!(p.get(&who("a", "t"), "junk", 0), Err(HostError::Invalid));
}

#[test]
fn a_running_job_is_running_and_a_failed_one_carries_its_error() {
    let (p, gate) = rig(64 << 20);
    let slow = p.submit(&who("a", "t"), "idx", b"slow").expect("starts");
    assert_eq!(
        p.get(&who("a", "t"), &slow, 0).expect("reads").state,
        JobState::Running
    );
    *gate.0.lock().expect("gate") = true;
    gate.1.notify_all();
    assert_eq!(
        p.get(&who("a", "t"), &slow, 5_000).expect("reads").state,
        JobState::Succeeded
    );
    let bad = p.submit(&who("a", "t"), "idx", b"error").expect("starts");
    let failed = p.get(&who("a", "t"), &bad, 5_000).expect("reads");
    assert_eq!(failed.state, JobState::Failed);
    assert!(String::from_utf8_lossy(&failed.body).contains("reason"));
}

#[test]
fn a_refused_search_starts_no_job_and_an_empty_pool_is_a_budget_refusal() {
    let (p, _) = rig(64 << 20);
    assert_eq!(
        p.submit(&who("a", "t"), "idx", b"refuse"),
        Err(HostError::Invalid)
    );
    assert_eq!(p.stats(&who("a", "t")).expect("stats").submitted, 0);
    let (empty, _) = rig(0);
    assert_eq!(
        empty.submit(&who("a", "t"), "idx", b"{}"),
        Err(HostError::Budget)
    );
}
