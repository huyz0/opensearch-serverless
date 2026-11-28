//! The host rules (docs/specs/plugin-system AC-4, AC-14): an operation needs
//! its capability, spends the call's budget, and answers within a ceiling.

use std::sync::Mutex;

use oss_ext::host::{Backend, CallHost, Host, HostError, NoBackend, NoHost, Who, operations};
use oss_ext::{Budget, Capability};

/// Answers every operation with its own name, and remembers what it was asked.
struct Echo(Mutex<Vec<String>>);

impl Backend for Echo {
    fn call(&self, who: &Who<'_>, op: &str, input: &[u8]) -> Result<Vec<u8>, HostError> {
        self.0
            .lock()
            .expect("log")
            .push(format!("{}/{}/{op}", who.plugin, who.tenant));
        let mut out = op.as_bytes().to_vec();
        out.extend_from_slice(input);
        Ok(out)
    }
}

fn who<'a>(grants: &'a [Capability]) -> Who<'a> {
    Who {
        plugin: "p",
        tenant: "t",
        grants,
    }
}

fn budget(state_ops: u32, searches: u32, egress: u32) -> Budget {
    Budget {
        state_ops,
        searches,
        egress,
        index_ops: 1,
    }
}

#[test]
fn an_operation_needs_its_capability() {
    let backend = Echo(Mutex::new(Vec::new()));
    let grants = [Capability::StateRead];
    let host = CallHost::new(who(&grants), budget(9, 9, 9), &backend);
    assert_eq!(host.call("state.get", b"k", 64), Ok(b"state.getk".to_vec()));
    for denied in [
        "state.put",
        "state.delete",
        "search",
        "job.get",
        "identity",
        "log",
    ] {
        assert_eq!(
            host.call(denied, b"", 64),
            Err(HostError::Denied),
            "{denied}"
        );
    }
    assert_eq!(host.call("nope", b"", 64), Err(HostError::Unknown));
    assert_eq!(*backend.0.lock().expect("log"), vec!["p/t/state.get"]);
}

#[test]
fn each_capability_opens_exactly_its_operations() {
    let cases: [(Capability, &[&str]); 9] = [
        (Capability::StateRead, &["state.get"]),
        (Capability::StateWrite, &["state.put", "state.delete"]),
        (Capability::Search, &["search"]),
        (
            Capability::Job,
            &["job.submit", "job.get", "job.delete", "job.stats"],
        ),
        (Capability::IndexWrite, &["index.docs"]),
        (Capability::Egress("a.example".into()), &["egress"]),
        (Capability::Identity, &["identity"]),
        (Capability::Log, &["log"]),
        (Capability::Metrics, &["metrics"]),
    ];
    let all: Vec<&str> = operations().collect();
    assert_eq!(all.len(), 13);
    for (cap, opens) in cases {
        let grants = [cap.clone()];
        let backend = Echo(Mutex::new(Vec::new()));
        let host = CallHost::new(who(&grants), budget(9, 9, 9), &backend);
        for op in &all {
            let got = host.call(op, b"", 64);
            if opens.contains(op) {
                assert!(got.is_ok(), "{cap:?} opens {op}");
            } else {
                assert_eq!(got, Err(HostError::Denied), "{cap:?} not {op}");
            }
        }
    }
}

#[test]
fn the_budget_is_per_kind_and_the_call_past_it_is_refused() {
    let backend = Echo(Mutex::new(Vec::new()));
    let grants = [
        Capability::StateRead,
        Capability::StateWrite,
        Capability::Search,
        Capability::Job,
        Capability::Egress("a.example".into()),
    ];
    let host = CallHost::new(who(&grants), budget(2, 1, 1), &backend);
    assert!(host.call("state.get", b"", 64).is_ok());
    assert!(
        host.call("state.put", b"", 64).is_ok(),
        "a put counts with a get"
    );
    assert_eq!(host.call("state.delete", b"", 64), Err(HostError::Budget));
    assert!(host.call("search", b"", 64).is_ok());
    assert_eq!(host.call("search", b"", 64), Err(HostError::Budget));
    assert_eq!(
        host.call("job.submit", b"", 64),
        Err(HostError::Budget),
        "a submit is a search"
    );
    for free in ["job.get", "job.delete", "job.stats"] {
        assert!(host.call(free, b"", 64).is_ok(), "{free} spends nothing");
    }
    assert!(host.call("egress", b"", 64).is_ok());
    assert_eq!(host.call("egress", b"", 64), Err(HostError::Budget));
    assert_eq!(
        backend.0.lock().expect("log").len(),
        7,
        "a refused call never reaches the backend"
    );
}

#[test]
fn a_zero_budget_refuses_the_first_call() {
    let grants = [Capability::Search];
    let backend = Echo(Mutex::new(Vec::new()));
    let host = CallHost::new(who(&grants), Budget::default(), &backend);
    assert_eq!(host.call("search", b"", 64), Err(HostError::Budget));
}

#[test]
fn an_answer_over_the_ceiling_is_refused_and_one_at_it_is_not() {
    let grants = [Capability::Identity];
    let backend = Echo(Mutex::new(Vec::new()));
    let host = CallHost::new(who(&grants), budget(0, 0, 0), &backend);
    assert_eq!(
        host.call("identity", b"", 8),
        Ok(b"identity".to_vec()),
        "exactly 8"
    );
    assert_eq!(host.call("identity", b"", 7), Err(HostError::TooLarge));
}

#[test]
fn nothing_behind_an_operation_is_unavailable_and_no_host_denies() {
    let grants = [Capability::Log];
    let host = CallHost::new(who(&grants), budget(0, 0, 0), &NoBackend);
    assert_eq!(host.call("log", b"", 8), Err(HostError::Unavailable));
    assert_eq!(NoHost.call("log", b"", 8), Err(HostError::Denied));
}

#[test]
fn each_error_has_its_status() {
    for (e, s) in [
        (HostError::Unknown, 400),
        (HostError::Invalid, 400),
        (HostError::Denied, 403),
        (HostError::Budget, 429),
        (HostError::TooLarge, 413),
        (HostError::Unavailable, 503),
        (HostError::Failed, 500),
    ] {
        assert_eq!(e.status(), s, "{e:?}");
    }
}
