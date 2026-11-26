//! docs/specs/snapshot-global-state AC-10 to AC-16: a restore that asks for
//! the global state puts its documents back by name and merges its settings.

use crate::snapshot_restore::{Rig, requests, rig_full, send};
use crate::snapshot_take::TAKE;

use std::time::{Duration, Instant};

use oss_blob::BlobStore;
use oss_format::{DocKind, MAX_DOCS};
use oss_meta::{ClusterConfig, DocSet};
use oss_node::restore_background::Settings;
use oss_node::stock_restore_route::StockRestoreSettings;
use oss_testkit::budget;
use std::sync::Arc;

const RESTORE: &str = "/_snapshot/backups/g/_restore?wait_for_completion=true";
const SCRIPT: &str = r#"{"script":{"lang":"painless","source":"1"}}"#;
const TEMPLATE: &str = r#"{"index_patterns":["x-*"]}"#;

fn blobs(r: &Rig) -> Arc<dyn BlobStore> {
    Arc::clone(&r.store) as Arc<dyn BlobStore>
}

fn docs(r: &Rig, kind: DocKind) -> DocSet {
    DocSet::new(blobs(r), "", kind).unwrap()
}

fn put(r: &Rig, kind: DocKind, name: &str, body: &str) {
    docs(r, kind)
        .put(name, body.as_bytes(), false, &mut vec![0u8; 1 << 19])
        .unwrap();
}

fn names(r: &Rig, kind: DocKind) -> Vec<String> {
    docs(r, kind).names(&mut vec![0u8; 1 << 19]).unwrap()
}

fn get(r: &Rig, kind: DocKind, name: &str) -> Option<String> {
    let mut body = vec![0u8; 1 << 20];
    let n = docs(r, kind)
        .get(name, &mut vec![0u8; 1 << 19], &mut body)
        .unwrap()?;
    Some(String::from_utf8(body[..n].to_vec()).unwrap())
}

fn settings(r: &Rig) -> Vec<(String, String)> {
    ClusterConfig::new(blobs(r), "")
        .unwrap()
        .read(&mut vec![0u8; 1 << 20])
        .unwrap()
        .settings
        .settings()
        .iter()
        .map(|(k, v)| (k.clone(), format!("{v:?}")))
        .collect()
}

fn rig_on(background: Settings) -> Rig {
    let r = rig_full(
        true,
        "",
        64 << 20,
        64,
        StockRestoreSettings::off(),
        background,
    );
    let body =
        r#"{"type":"fs","settings":{"location":"x","remote_store_index_shallow_copy":true}}"#;
    assert_eq!(
        send(&r, "PUT", "/_snapshot/backups", Some(body)).status,
        200
    );
    crate::snapshot_restore::live_index(&r, "logs", "u1", 2, b"payload");
    r
}

fn state(r: &Rig) {
    ClusterConfig::new(blobs(r), "")
        .unwrap()
        .update(
            &[("cluster.a", Some("1")), ("cluster.b", Some("true"))],
            &mut vec![0u8; 1 << 20],
        )
        .unwrap();
    put(r, DocKind::IndexTemplate, "t", TEMPLATE);
    put(r, DocKind::StoredScript, "s", SCRIPT);
}

fn take_g(r: &Rig) {
    let body = r#"{"indices":"logs","include_global_state":true}"#;
    let made = send(
        r,
        "PUT",
        "/_snapshot/backups/g?wait_for_completion=true",
        Some(body),
    );
    assert_eq!(made.status, 200, "{}", made.text());
}

fn restore(r: &Rig, body: &str) -> crate::support::Reply {
    send(r, "POST", RESTORE, Some(body))
}

// AC-10: the documents come back by name, the settings merge, nothing is deleted.
#[test]
fn a_restore_puts_the_documents_back_and_merges_the_settings_without_deleting() {
    let r = rig_on(Settings::off());
    state(&r);
    take_g(&r);
    put(
        &r,
        DocKind::StoredScript,
        "s",
        r#"{"script":{"lang":"painless","source":"2"}}"#,
    );
    put(&r, DocKind::StoredScript, "added-since", SCRIPT);
    docs(&r, DocKind::IndexTemplate)
        .delete("t", &mut vec![0u8; 1 << 19])
        .unwrap();
    ClusterConfig::new(blobs(&r), "")
        .unwrap()
        .update(
            &[("cluster.a", Some("9")), ("cluster.extra", Some("x"))],
            &mut vec![0u8; 1 << 20],
        )
        .unwrap();
    let got = restore(
        &r,
        r#"{"rename_pattern":"logs","rename_replacement":"copy","include_global_state":true}"#,
    );
    assert_eq!(got.status, 200, "{}", got.text());
    assert!(
        got.text().contains(r#""global_state_restored":4"#),
        "{}",
        got.text()
    );
    assert!(
        !got.text().contains("failed_global_state"),
        "{}",
        got.text()
    );
    assert_eq!(
        get(&r, DocKind::IndexTemplate, "t").as_deref(),
        Some(TEMPLATE)
    );
    assert!(
        get(&r, DocKind::StoredScript, "s")
            .unwrap()
            .contains(r#""source":"1""#)
    );
    assert!(
        get(&r, DocKind::StoredScript, "added-since").is_some(),
        "nothing is deleted"
    );
    let kept = settings(&r);
    assert!(
        kept.iter()
            .any(|(k, v)| k == "cluster.a" && v.contains('1')),
        "{kept:?}"
    );
    assert!(kept.iter().any(|(k, _)| k == "cluster.extra"), "{kept:?}");
    assert!(kept.iter().any(|(k, _)| k == "cluster.b"), "{kept:?}");
}

// AC-11: without the flag nothing is applied and the answer has no report.
#[test]
fn without_the_flag_the_state_is_left_alone() {
    let r = rig_on(Settings::off());
    state(&r);
    take_g(&r);
    docs(&r, DocKind::StoredScript)
        .delete("s", &mut vec![0u8; 1 << 19])
        .unwrap();
    let got = restore(
        &r,
        r#"{"rename_pattern":"logs","rename_replacement":"copy"}"#,
    );
    assert_eq!(got.status, 200, "{}", got.text());
    assert!(!got.text().contains("global_state"), "{}", got.text());
    assert!(names(&r, DocKind::StoredScript).is_empty());
}

// AC-12: a snapshot that holds no state answers zero; one with no indices selected applies only the state.
#[test]
fn a_snapshot_without_state_answers_zero_and_no_indices_applies_only_the_state() {
    let r = rig_on(Settings::off());
    let plain = send(
        &r,
        "PUT",
        "/_snapshot/backups/g?wait_for_completion=true",
        Some(TAKE),
    );
    assert_eq!(plain.status, 200, "{}", plain.text());
    let none = restore(
        &r,
        r#"{"rename_pattern":"logs","rename_replacement":"copy","include_global_state":true}"#,
    );
    assert_eq!(none.status, 200, "{}", none.text());
    assert!(
        none.text().contains(r#""global_state_restored":0"#),
        "{}",
        none.text()
    );

    let r = rig_on(Settings::off());
    state(&r);
    take_g(&r);
    docs(&r, DocKind::StoredScript)
        .delete("s", &mut vec![0u8; 1 << 19])
        .unwrap();
    let only = restore(&r, r#"{"indices":[],"include_global_state":true}"#);
    assert_eq!(only.status, 200, "{}", only.text());
    assert!(only.text().contains(r#""indices":[]"#), "{}", only.text());
    assert!(
        only.text().contains(r#""global_state_restored":4"#),
        "{}",
        only.text()
    );
    assert!(get(&r, DocKind::StoredScript, "s").is_some());
}

// AC-13: a document that no longer validates is named, the rest are applied, the indices stay.
#[test]
fn a_document_that_no_longer_validates_is_named_and_the_rest_are_applied() {
    let r = rig_on(Settings::off());
    state(&r);
    put(
        &r,
        DocKind::StoredScript,
        "bad",
        r#"{"script":{"lang":"nope","source":"1"}}"#,
    );
    take_g(&r);
    docs(&r, DocKind::StoredScript)
        .delete("s", &mut vec![0u8; 1 << 19])
        .unwrap();
    docs(&r, DocKind::StoredScript)
        .delete("bad", &mut vec![0u8; 1 << 19])
        .unwrap();
    let got = restore(
        &r,
        r#"{"rename_pattern":"logs","rename_replacement":"copy","include_global_state":true}"#,
    );
    assert_eq!(got.status, 200, "{}", got.text());
    assert!(
        got.text()
            .contains(r#""failed_global_state":["stored_script/bad"]"#),
        "{}",
        got.text()
    );
    assert!(
        got.text().contains(r#""global_state_restored":4"#),
        "{}",
        got.text()
    );
    assert!(get(&r, DocKind::StoredScript, "s").is_some());
    assert!(get(&r, DocKind::StoredScript, "bad").is_none());
    assert!(
        r.descriptors
            .get("copy", &mut vec![0u8; 1 << 16])
            .unwrap()
            .is_some()
    );
}

fn fill(r: &Rig, count: usize) {
    for n in 0..count {
        put(
            r,
            DocKind::IngestPipeline,
            &format!("p{n}"),
            r#"{"processors":[]}"#,
        );
    }
}

// AC-14: a set that would pass 1,000 documents is refused 400 before anything is written;
// exactly 1,000 is applied.
#[test]
fn a_set_past_a_thousand_documents_is_refused_and_exactly_a_thousand_is_not() {
    let r = rig_on(Settings::off());
    fill(&r, MAX_DOCS);
    take_g(&r);
    // The snapshot's own thousand are all held: nothing is added, so this is allowed.
    let same = restore(
        &r,
        r#"{"rename_pattern":"logs","rename_replacement":"same","include_global_state":true}"#,
    );
    assert_eq!(same.status, 200, "{}", same.text());
    assert!(
        same.text().contains(r#""global_state_restored":1000"#),
        "{}",
        same.text()
    );
    // One name swapped: held 999 of the snapshot's names plus a new one makes 1,001.
    docs(&r, DocKind::IngestPipeline)
        .delete("p0", &mut vec![0u8; 1 << 19])
        .unwrap();
    put(&r, DocKind::IngestPipeline, "other", r#"{"processors":[]}"#);
    let before = names(&r, DocKind::IngestPipeline);
    let over = restore(
        &r,
        r#"{"rename_pattern":"logs","rename_replacement":"two","include_global_state":true}"#,
    );
    assert_eq!(over.status, 400, "{}", over.text());
    assert!(
        over.text().contains("illegal_argument_exception"),
        "{}",
        over.text()
    );
    assert!(over.text().contains("ingest_pipelines"), "{}", over.text());
    assert_eq!(names(&r, DocKind::IngestPipeline), before);
    assert!(
        r.descriptors
            .get("two", &mut vec![0u8; 1 << 16])
            .unwrap()
            .is_none()
    );
}

// AC-15: a background restore applies the state after its indices are whole.
#[test]
fn a_background_restore_applies_the_state_after_the_indices() {
    let r = rig_on(Settings {
        enabled: true,
        max: 1,
        grace: Duration::from_secs(1),
    });
    state(&r);
    take_g(&r);
    docs(&r, DocKind::StoredScript)
        .delete("s", &mut vec![0u8; 1 << 19])
        .unwrap();
    let got = send(
        &r,
        "POST",
        "/_snapshot/backups/g/_restore",
        Some(
            r#"{"rename_pattern":"logs","rename_replacement":"copy","include_global_state":true}"#,
        ),
    );
    assert_eq!(got.status, 200, "{}", got.text());
    let start = Instant::now();
    while (r.restores.running() > 0 || get(&r, DocKind::StoredScript, "s").is_none())
        && start.elapsed() < Duration::from_secs(10)
    {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(get(&r, DocKind::StoredScript, "s").is_some());
    assert_eq!(r.restores.failed(), 0);
}

// AC-16: what a restore of the state costs.
#[test]
fn a_restore_of_the_state_costs_what_the_budget_holds() {
    let r = rig_on(Settings::off());
    let plain = send(
        &r,
        "PUT",
        "/_snapshot/backups/g?wait_for_completion=true",
        Some(TAKE),
    );
    assert_eq!(plain.status, 200, "{}", plain.text());
    let before = requests(&r);
    let none = restore(&r, r#"{"indices":[],"include_global_state":true}"#);
    assert_eq!(none.status, 200, "{}", none.text());
    budget::check(
        "snapshot.restore_global_state_empty.requests",
        requests(&r) - before,
        "a restore selecting no index, of a snapshot with no global state",
    );

    let r = rig_on(Settings::off());
    state(&r);
    take_g(&r);
    let before = requests(&r);
    let one = restore(&r, r#"{"indices":[],"include_global_state":true}"#);
    assert_eq!(one.status, 200, "{}", one.text());
    budget::check(
        "snapshot.restore_global_state_one_doc.requests",
        requests(&r) - before,
        "a restore selecting no index, of a state of one template, one script and two settings",
    );
}
