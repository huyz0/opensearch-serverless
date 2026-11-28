//! docs/specs/plugin-system Host API 1.0: the `job.*` operations, as bytes.

use std::sync::{Arc, Mutex};

use oss_ext::host::{Backend, HostError, NoBackend, Who};
use oss_ext::job::{
    JobBackend, JobSink, JobState, JobStats, JobView, MAX_BODY_BYTES, decode_delete, decode_get,
    decode_id, decode_stats, decode_submit, decode_view, encode_delete, encode_get, encode_stats,
    encode_submit, encode_view,
};
use oss_ext::rest::CodecError;

/// Remembers what it was asked and answers fixed values.
#[derive(Default)]
struct Sink(Mutex<Vec<String>>);

impl JobSink for Sink {
    fn submit(&self, who: &Who<'_>, index: &str, body: &[u8]) -> Result<String, HostError> {
        self.0.lock().expect("log").push(format!(
            "submit {}/{}/{index}/{}",
            who.plugin,
            who.tenant,
            body.len()
        ));
        if index == "full" {
            return Err(HostError::Budget);
        }
        Ok("n1:7".into())
    }

    fn get(&self, _: &Who<'_>, id: &str, wait_ms: u64) -> Result<JobView, HostError> {
        self.0
            .lock()
            .expect("log")
            .push(format!("get {id} {wait_ms}"));
        if id == "n1:9" {
            return Err(HostError::Invalid);
        }
        Ok(JobView {
            state: JobState::Succeeded,
            body: b"{}".to_vec(),
        })
    }

    fn delete(&self, _: &Who<'_>, id: &str) -> Result<(), HostError> {
        self.0.lock().expect("log").push(format!("delete {id}"));
        Ok(())
    }

    fn stats(&self, _: &Who<'_>) -> Result<JobStats, HostError> {
        Ok(JobStats {
            running: 1,
            submitted: 2,
            cancelled: 3,
            rejected: 4,
        })
    }
}

fn who() -> Who<'static> {
    Who {
        plugin: "p",
        tenant: "t",
        grants: &[],
    }
}

fn backend() -> (JobBackend, Arc<Sink>) {
    let sink = Arc::new(Sink::default());
    (
        JobBackend::new(Arc::clone(&sink) as Arc<dyn JobSink>, Arc::new(NoBackend)),
        sink,
    )
}

#[test]
fn every_call_and_answer_round_trips() {
    assert_eq!(
        decode_submit(&encode_submit("logs-*", b"{}")).expect("submit"),
        ("logs-*".to_owned(), b"{}".to_vec())
    );
    assert_eq!(
        decode_get(&encode_get("n1:7", 250)).expect("get"),
        ("n1:7".to_owned(), 250)
    );
    assert_eq!(
        decode_delete(&encode_delete("n1:7")).expect("delete"),
        "n1:7"
    );
    let view = JobView {
        state: JobState::Failed,
        body: b"boom".to_vec(),
    };
    assert_eq!(decode_view(&encode_view(&view)).expect("view"), view);
    let stats = JobStats {
        running: 1,
        submitted: 2,
        cancelled: 3,
        rejected: 4,
    };
    assert_eq!(decode_stats(&encode_stats(&stats)).expect("stats"), stats);
}

#[test]
fn bytes_that_are_not_a_call_are_refused() {
    let good = encode_submit("i", b"{}");
    assert_eq!(
        decode_submit(&good[..good.len() - 1]),
        Err(CodecError::Short)
    );
    let mut other = good.clone();
    other[0] = 2;
    assert_eq!(decode_submit(&other), Err(CodecError::Version));
    let mut long = good;
    long.push(0);
    assert_eq!(decode_submit(&long), Err(CodecError::Trailing));
    let huge = encode_submit("i", &vec![b' '; MAX_BODY_BYTES + 1]);
    assert_eq!(decode_submit(&huge), Err(CodecError::Bound));
    let mut state = encode_view(&JobView {
        state: JobState::Running,
        body: Vec::new(),
    });
    state[1] = 9;
    assert_eq!(decode_view(&state), Err(CodecError::Value));
}

#[test]
fn the_backend_serves_the_job_operations_and_defers_the_rest() {
    let (b, sink) = backend();
    let id = b
        .call(&who(), "job.submit", &encode_submit("logs", b"{}"))
        .expect("submit");
    assert_eq!(decode_id(&id).expect("id"), "n1:7");
    let got = b
        .call(&who(), "job.get", &encode_get("n1:7", 5))
        .expect("get");
    assert_eq!(decode_view(&got).expect("view").state, JobState::Succeeded);
    b.call(&who(), "job.delete", &encode_delete("n1:7"))
        .expect("delete");
    let stats = b.call(&who(), "job.stats", &[1]).expect("stats");
    assert_eq!(decode_stats(&stats).expect("stats").submitted, 2);
    assert_eq!(
        *sink.0.lock().expect("log"),
        ["submit p/t/logs/2", "get n1:7 5", "delete n1:7"]
    );
    // Another operation reaches the backend behind, which here has nothing.
    assert_eq!(
        b.call(&who(), "state.get", &[]),
        Err(HostError::Unavailable)
    );
}

#[test]
fn a_refusal_of_the_sink_and_bad_bytes_are_the_hosts_errors() {
    let (b, _) = backend();
    assert_eq!(
        b.call(&who(), "job.submit", &encode_submit("full", b"{}")),
        Err(HostError::Budget)
    );
    assert_eq!(
        b.call(&who(), "job.get", &encode_get("n1:9", 0)),
        Err(HostError::Invalid)
    );
    assert_eq!(
        b.call(&who(), "job.get", &[1, 255]),
        Err(HostError::Invalid)
    );
    assert_eq!(b.call(&who(), "job.stats", &[]), Err(HostError::Invalid));
    assert_eq!(
        b.call(&who(), "job.stats", &[1, 0]),
        Err(HostError::Invalid)
    );
}
