//! docs/specs/anomaly-detection slice 4: `_start` and `_stop`, the job record
//! and the schedule registry's `ad` namespace.

use std::sync::Arc;

use oss_blob::{BlobStore, S3Op};
use oss_meta::ScheduleRegistry;
use serde_json::{Value, json};

use super::ad_routes::{ROOT, Rig, call, create, detector, json_of, rig_with, total};

fn job_of(r: &Rig, id: &str) -> Value {
    let got = call(r, "GET", &format!("{ROOT}/detectors/{id}?job=true"), None);
    assert_eq!(got.status, 200, "{}", got.text());
    json_of(&got)["anomaly_detector_job"].clone()
}

fn registry(r: &Rig, namespace: &str) -> ScheduleRegistry {
    let store: Arc<dyn BlobStore> = Arc::clone(&r.store) as Arc<dyn BlobStore>;
    ScheduleRegistry::in_namespace(store, "", namespace).expect("a registry")
}

// slice 4a has no criterion of its own; AC-8 and AC-9 come with the runner
#[test]
fn a_start_enables_the_job_and_names_the_tenant_a_stop_disables_it() {
    let r = rig_with(None);
    let id = create(&r, &detector());
    assert!(
        job_of(&r, &id).is_null(),
        "a created detector is not running"
    );
    assert!(!registry(&r, "ad").is_registered("_default_").unwrap());

    let started = call(&r, "POST", &format!("{ROOT}/detectors/{id}/_start"), None);
    assert_eq!(started.status, 200, "{}", started.text());
    assert_eq!(json_of(&started)["_id"], id);
    let job = job_of(&r, &id);
    assert_eq!(job["enabled"], true);
    assert_eq!(job["name"], id.as_str());
    assert!(job["enabled_time"].as_i64().is_some_and(|t| t > 0));
    assert!(job["disabled_time"].is_null());
    assert_eq!(
        job["schedule"]["interval"]["period"], 10,
        "the detector's detection interval, in minutes"
    );
    assert!(registry(&r, "ad").is_registered("_default_").unwrap());
    assert!(
        !registry(&r, "alerting").is_registered("_default_").unwrap(),
        "the namespaces are apart"
    );

    let stopped = call(&r, "POST", &format!("{ROOT}/detectors/{id}/_stop"), None);
    assert_eq!(stopped.status, 200, "{}", stopped.text());
    assert_eq!(json_of(&stopped), json!(format!("Stopped detector: {id}")));
    let job = job_of(&r, &id);
    assert_eq!(job["enabled"], false);
    assert!(job["disabled_time"].as_i64().is_some_and(|t| t > 0));
}

#[test]
fn a_start_of_a_running_detector_and_a_stop_of_a_stopped_one_change_nothing() {
    let r = rig_with(None);
    let id = create(&r, &detector());
    let start = |r: &Rig| call(r, "POST", &format!("{ROOT}/detectors/{id}/_start"), None);
    let first = json_of(&start(&r));
    let (gets, puts) = (total(&r, S3Op::Get), total(&r, S3Op::Put));
    let again = json_of(&start(&r));
    assert_eq!(again["_seq_no"], first["_seq_no"], "no second write");
    assert_eq!(
        total(&r, S3Op::Put) - puts,
        1,
        "a repeat start writes only the registry's swap"
    );
    assert!(total(&r, S3Op::Get) > gets);

    let id = create(&r, &detector());
    let (puts, gets) = (total(&r, S3Op::Put), total(&r, S3Op::Get));
    let stopped = call(&r, "POST", &format!("{ROOT}/detectors/{id}/_stop"), None);
    assert_eq!(stopped.status, 200, "{}", stopped.text());
    assert_eq!(
        total(&r, S3Op::Put),
        puts,
        "a stop of a stopped detector writes nothing"
    );
    assert_eq!(total(&r, S3Op::Get) - gets, 2, "it reads the detector");
    assert_eq!(total(&r, S3Op::List), 0);
}

#[test]
fn a_replace_keeps_the_job_and_a_start_of_nothing_is_a_404() {
    let r = rig_with(None);
    let id = create(&r, &detector());
    call(&r, "POST", &format!("{ROOT}/detectors/{id}/_start"), None);
    let put = call(
        &r,
        "PUT",
        &format!("{ROOT}/detectors/{id}"),
        Some(&detector()),
    );
    assert_eq!(put.status, 200, "{}", put.text());
    assert_eq!(job_of(&r, &id)["enabled"], true, "a replace keeps the job");

    for tail in ["_start", "_stop"] {
        let missing = call(&r, "POST", &format!("{ROOT}/detectors/nope/{tail}"), None);
        assert_eq!(missing.status, 404, "{tail}: {}", missing.text());
    }
}

#[test]
fn a_start_with_a_range_is_a_historical_analysis_which_is_not_served_yet() {
    let r = rig_with(None);
    let id = create(&r, &detector());
    let body = json!({"start_time": 1, "end_time": 2});
    let refused = call(
        &r,
        "POST",
        &format!("{ROOT}/detectors/{id}/_start"),
        Some(&body),
    );
    assert_eq!(refused.status, 400, "{}", refused.text());
    assert!(job_of(&r, &id).is_null(), "nothing was started");
    assert!(!registry(&r, "ad").is_registered("_default_").unwrap());
}

#[test]
fn a_profile_says_disabled_then_init_and_refuses_what_it_cannot_answer() {
    let rig = rig_with(None);
    let id = create(&rig, &detector());
    let profile = |tail: &str| {
        call(
            &rig,
            "GET",
            &format!("{ROOT}/detectors/{id}/_profile{tail}"),
            None,
        )
    };
    assert_eq!(json_of(&profile(""))["state"], "DISABLED");
    let started = call(&rig, "POST", &format!("{ROOT}/detectors/{id}/_start"), None);
    assert_eq!(started.status, 200, "{}", started.text());
    // its first slot is not due yet, so no run has been offered
    assert_eq!(json_of(&profile("/state"))["state"], "INIT");
    assert_eq!(profile("/models").status, 400);
    let missing = call(
        &rig,
        "GET",
        &format!("{ROOT}/detectors/nope/_profile"),
        None,
    );
    assert_eq!(missing.status, 404);
}
