//! The registry's bounds, with the clock and the work's gates passed in
//! (docs/specs/async-search AC-2, 5, 6, 7, 8, 9, 12, 17).

use std::sync::atomic::AtomicU64;
use std::sync::{Condvar, Mutex};

use oss_mem::{BudgetConfig, Pool};

use super::*;

/// A door a search waits at until the test opens it.
#[derive(Default)]
struct Gate(Mutex<bool>, Condvar);

impl Gate {
    fn open(&self) {
        *self.0.lock().expect("gate") = true;
        self.1.notify_all();
    }

    fn wait(&self) {
        let mut open = self.0.lock().expect("gate");
        while !*open {
            open = self.1.wait(open).expect("gate");
        }
    }
}

fn who(t: &str) -> Who {
    Who {
        tenant: t.into(),
        user: "u".into(),
    }
}

fn rig(limits: Limits) -> (Arc<Jobs>, MemoryBudget, Arc<AtomicU64>) {
    let config = BudgetConfig::new(256 << 20)
        .with_pool(Pool::Extensions, 128 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let now = Arc::new(AtomicU64::new(1_000));
    let c = Arc::clone(&now);
    let clock: Clock = Arc::new(move || c.load(Ordering::Relaxed));
    (Jobs::new("n1", &budget, limits, clock), budget, now)
}

/// A kept job that is not waited on, as these tests read it afterwards.
fn sub(jobs: &Arc<Jobs>, who: &Who, work: Work, keep_alive: Option<u64>) -> Result<u64, JobError> {
    jobs.submit(who, work, keep_alive, true, String::new(), 0)
        .map(|(n, _)| n)
}

fn instant(body: &'static str) -> Work {
    Box::new(move |_| Ok(body.as_bytes().to_vec()))
}

fn held(gate: &Arc<Gate>) -> Work {
    let g = Arc::clone(gate);
    Box::new(move |_| {
        g.wait();
        Ok(b"late".to_vec())
    })
}

fn settle(jobs: &Jobs, budget: &MemoryBudget) {
    for _ in 0..500 {
        if jobs.lock().threads == 0
            && budget.in_use(Pool::Extensions)
                == jobs
                    .lock()
                    .entries
                    .iter()
                    .map(|e| e.held.as_ref().map_or(0, Reservation::bytes))
                    .sum::<usize>()
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("threads did not settle");
}

// AC-2: a search that finishes inside the wait is read as succeeded; one held
// past it is running, and a read never restarts it.
#[test]
fn a_read_waits_for_the_result_and_never_restarts_it() {
    let (jobs, _, _) = rig(Limits::default());
    let gate = Arc::new(Gate::default());
    let a = sub(&jobs, &who("t"), instant("{}"), None).expect("room");
    let v = jobs.get(&who("t"), a, 5_000).expect("job");
    assert_eq!(
        (v.state, v.body.as_deref()),
        (State::Succeeded, Some(&b"{}"[..]))
    );
    let b = sub(&jobs, &who("t"), held(&gate), None).expect("room");
    assert_eq!(
        jobs.get(&who("t"), b, 20).expect("job").state,
        State::Running
    );
    gate.open();
    assert_eq!(
        jobs.get(&who("t"), b, 5_000).expect("job").state,
        State::Succeeded
    );
}

// AC-5: keep-alive bounds, default, and expiry on access with no timer.
#[test]
fn keep_alive_is_bounded_and_expiry_runs_on_access() {
    let (jobs, _, now) = rig(Limits::default());
    let err = sub(&jobs, &who("t"), instant("{}"), Some(3_600_001)).expect_err("too large");
    assert_eq!(err.status(), 400);
    assert_eq!(jobs.limits().default_keep_alive_ms(), 3_600_000);
    let a = sub(&jobs, &who("t"), instant("{}"), Some(1_000)).expect("room");
    jobs.get(&who("t"), a, 5_000).expect("finished");
    now.store(1_000 + 999, Ordering::Relaxed);
    assert!(jobs.get(&who("t"), a, 0).is_ok());
    jobs.extend(&who("t"), a, 5_000).expect("extend");
    now.store(1_000 + 999 + 4_999, Ordering::Relaxed);
    assert!(jobs.get(&who("t"), a, 0).is_ok());
    now.store(1_000 + 999 + 5_000, Ordering::Relaxed);
    assert_eq!(jobs.get(&who("t"), a, 0).err(), Some(JobError::NotFound));
    assert_eq!(jobs.stats().expired, 1);
}

// AC-7: the running limits, the tenant's, and nothing queued.
#[test]
fn running_limits_refuse_the_next_submit() {
    let (jobs, _, _) = rig(Limits::default());
    let gate = Arc::new(Gate::default());
    for t in ["a", "a", "b", "b", "c", "c", "d", "d"] {
        sub(&jobs, &who(t), held(&gate), None).expect("room");
    }
    assert_eq!(
        sub(&jobs, &who("e"), held(&gate), None)
            .err()
            .map(|e| e.status()),
        Some(429)
    );
    gate.open();
    let (jobs, _, _) = rig(Limits::default());
    let gate = Arc::new(Gate::default());
    sub(&jobs, &who("a"), held(&gate), None).expect("room");
    sub(&jobs, &who("a"), held(&gate), None).expect("room");
    assert_eq!(
        sub(&jobs, &who("a"), held(&gate), None)
            .err()
            .map(|e| e.status()),
        Some(429)
    );
    assert!(sub(&jobs, &who("b"), held(&gate), None).is_ok());
    assert_eq!(jobs.stats().rejected, 1);
    gate.open();
}

// AC-6, AC-8: a delete cancels; the slot and the reservation stay until the
// thread exits.
#[test]
fn a_cancelled_search_keeps_its_slot_and_share_until_its_thread_exits() {
    let (jobs, budget, _) = rig(Limits::default());
    let stopped = Arc::new(Gate::default());
    let seen = Arc::new(Gate::default());
    let (s, g) = (Arc::clone(&stopped), Arc::clone(&seen));
    let work: Work = Box::new(move |ctx| {
        while !ctx.should_stop() {
            std::thread::sleep(Duration::from_millis(1));
        }
        g.open();
        s.wait();
        Err(Failure::Stopped)
    });
    let a = sub(&jobs, &who("t"), work, None).expect("room");
    let per = Limits::default().max_response_bytes + ENTRY_OVERHEAD;
    assert_eq!(budget.in_use(Pool::Extensions), per);
    jobs.delete(&who("t"), a).expect("cancel");
    seen.wait();
    assert_eq!(jobs.get(&who("t"), a, 0).err(), Some(JobError::NotFound));
    assert_eq!(
        budget.in_use(Pool::Extensions),
        per,
        "held while the thread runs"
    );
    assert_eq!(jobs.stats().running_current, 1);
    stopped.open();
    settle(&jobs, &budget);
    assert_eq!(budget.in_use(Pool::Extensions), 0);
    assert_eq!(jobs.stats().cancelled, 1);
}

// AC-7: a result over the cap is a failure, never held.
#[test]
fn a_result_over_the_cap_fails_and_holds_nothing() {
    let limits = Limits {
        max_response_bytes: 8,
        ..Limits::default()
    };
    let (jobs, budget, _) = rig(limits);
    let a = sub(&jobs, &who("t"), Box::new(|_| Ok(vec![0; 9])), None).expect("room");
    let v = jobs.get(&who("t"), a, 5_000).expect("job");
    assert_eq!(
        (v.state, v.error.map(|e| e.0)),
        (State::Failed, Some("response_too_large"))
    );
    let b = sub(&jobs, &who("t"), Box::new(|_| Err(Failure::TooLarge)), None).expect("room");
    assert_eq!(
        jobs.get(&who("t"), b, 5_000)
            .expect("job")
            .error
            .map(|e| e.0),
        Some("response_too_large")
    );
    settle(&jobs, &budget);
    assert!(budget.in_use(Pool::Extensions) <= 2 * (ENTRY_OVERHEAD + 8));
}

// AC-8: a kept result holds its size plus the overhead, released with the entry.
#[test]
fn a_kept_result_holds_its_size_and_gives_it_back() {
    let (jobs, budget, _) = rig(Limits::default());
    let a = sub(&jobs, &who("t"), instant("12345"), None).expect("room");
    jobs.get(&who("t"), a, 5_000).expect("job");
    settle(&jobs, &budget);
    assert_eq!(budget.in_use(Pool::Extensions), 5 + ENTRY_OVERHEAD);
    jobs.delete(&who("t"), a).expect("drop");
    assert_eq!(budget.in_use(Pool::Extensions), 0);
}

// AC-9: a panic frees the slot and the share and reads failed.
#[test]
fn a_panicking_search_is_a_failed_job_and_frees_its_slot() {
    let (jobs, budget, _) = rig(Limits::default());
    let a = sub(&jobs, &who("t"), Box::new(|_| panic!("boom")), None).expect("room");
    let v = jobs.get(&who("t"), a, 5_000).expect("job");
    assert_eq!(v.error.map(|e| e.0), Some("panic"));
    settle(&jobs, &budget);
    assert_eq!(jobs.stats().running_current, 0);
    assert!(sub(&jobs, &who("t"), instant("{}"), None).is_ok());
}

// AC-12: another tenant or another user of the tenant sees 404.
#[test]
fn a_job_is_the_submitters() {
    let (jobs, _, _) = rig(Limits::default());
    let a = sub(&jobs, &who("t"), instant("{}"), None).expect("room");
    let other_user = Who {
        tenant: "t".into(),
        user: "v".into(),
    };
    for w in [who("x"), other_user] {
        assert_eq!(jobs.get(&w, a, 0).err(), Some(JobError::NotFound));
        assert_eq!(jobs.extend(&w, a, 1).err(), Some(JobError::NotFound));
        assert_eq!(jobs.delete(&w, a).err(), Some(JobError::NotFound));
    }
}

// AC-7: oldest finished first, a tenant's own first, never a running one.
#[test]
fn eviction_takes_the_oldest_finished_of_the_tenant_over_its_half() {
    let limits = Limits {
        max_kept: 4,
        max_running_per_tenant: 8,
        ..Limits::default()
    };
    let (jobs, budget, now) = rig(limits);
    let mut ids = Vec::new();
    for _ in 0..3 {
        now.fetch_add(1, Ordering::Relaxed);
        let id = sub(&jobs, &who("a"), instant("{}"), None).expect("room");
        jobs.get(&who("a"), id, 5_000).expect("job");
        settle(&jobs, &budget);
        ids.push(id);
    }
    assert_eq!(
        jobs.get(&who("a"), ids[0], 0).err(),
        Some(JobError::NotFound)
    );
    assert!(jobs.get(&who("a"), ids[2], 0).is_ok());
    assert_eq!(jobs.stats().evicted, 1);
    let gate = Arc::new(Gate::default());
    sub(&jobs, &who("b"), held(&gate), None).expect("room");
    sub(&jobs, &who("b"), held(&gate), None).expect("room");
    assert_eq!(
        sub(&jobs, &who("b"), held(&gate), None)
            .err()
            .map(|e| e.status()),
        Some(429)
    );
    gate.open();
}

// AC-17: a search past its running time is told to stop and reads failed
// with nobody reading it in between.
#[test]
fn a_search_past_its_running_time_stops_by_itself() {
    let limits = Limits {
        max_running_time_ms: 500,
        ..Limits::default()
    };
    let (jobs, budget, now) = rig(limits);
    let work: Work = Box::new(|ctx| {
        while !ctx.should_stop() {
            std::thread::sleep(Duration::from_millis(1));
        }
        Err(Failure::Stopped)
    });
    let a = sub(&jobs, &who("t"), work, None).expect("room");
    now.store(1_000 + 500, Ordering::Relaxed);
    settle(&jobs, &budget);
    let v = jobs.get(&who("t"), a, 0).expect("job");
    assert_eq!(v.error.map(|e| e.0), Some("search_timeout"));
}

// AC-10 (the local half): ids name the node and never repeat.
#[test]
fn an_id_names_its_node_and_is_malformed_when_it_does_not() {
    let (jobs, _, _) = rig(Limits::default());
    assert_eq!(jobs.route("n1:7").expect("local"), Target::Local(7));
    assert_eq!(
        jobs.route("n2:7").expect("remote"),
        Target::Remote {
            node: "n2".into(),
            number: 7
        }
    );
    for bad in ["7", ":7", "n1:x", ""] {
        assert_eq!(
            jobs.route(bad).err().map(|e| e.status()),
            Some(400),
            "{bad}"
        );
    }
    let a = sub(&jobs, &who("t"), instant("{}"), None).expect("room");
    assert!(a >= 1_000, "ids start from the clock");
}

// docs/specs/async-search AC-4 (stock recording): a job that is not kept is
// answered to the submit that waited and is gone afterwards; one that
// finishes after the wait is gone as soon as it finishes; a kept one stays.
#[test]
fn a_job_that_is_not_kept_is_dropped_once_read_or_finished() {
    let (jobs, budget, _) = rig(Limits::default());
    let (n, v) = jobs
        .submit(&who("t"), instant("{}"), None, false, String::new(), 5_000)
        .expect("room");
    assert_eq!(v.state, State::Succeeded);
    assert_eq!(jobs.get(&who("t"), n, 0).err(), Some(JobError::NotFound));
    let gate = Arc::new(Gate::default());
    let (m, v) = jobs
        .submit(&who("t"), held(&gate), None, false, String::new(), 10)
        .expect("room");
    assert_eq!(v.state, State::Running);
    assert_eq!(
        jobs.get(&who("t"), m, 0).expect("running").state,
        State::Running
    );
    gate.open();
    settle(&jobs, &budget);
    assert_eq!(jobs.get(&who("t"), m, 0).err(), Some(JobError::NotFound));
    assert_eq!(
        budget.in_use(Pool::Extensions),
        0,
        "nothing held for a dropped job"
    );
    let (k, v) = jobs
        .submit(&who("t"), instant("{}"), None, true, String::new(), 5_000)
        .expect("room");
    assert_eq!(v.state, State::Succeeded);
    assert_eq!(
        jobs.get(&who("t"), k, 0).expect("kept").state,
        State::Succeeded
    );
}
