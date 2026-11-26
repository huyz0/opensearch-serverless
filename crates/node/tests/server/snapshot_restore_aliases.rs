//! docs/specs/snapshot-aliases AC-3 to AC-9: a take records the aliases of
//! its indices and a restore with `include_aliases` adds them to what it
//! creates, in the foreground and in the background.

use crate::snapshot_restore::{
    RESTORE, Rig, requests, rig_aliased, rig_full, send, taken_with_aliases,
};

use std::time::{Duration, Instant};

use oss_meta::{AliasClock, AliasMutations, AliasRecord, Target};
use oss_node::restore_background::Settings;
use oss_node::stock_restore_route::StockRestoreSettings;
use oss_testkit::budget;

const COPY: &str = r#""rename_pattern":"logs","rename_replacement":"copy""#;

fn body(extra: &str) -> String {
    format!("{{{COPY}{extra}}}")
}

fn alias_of(r: &Rig, name: &str) -> Option<(Vec<String>, Option<String>, bool)> {
    let mut buf = vec![0u8; 1 << 16];
    match r.descriptors.resolve(name, &mut buf).unwrap().target {
        Target::Alias(a) => Some((
            a.indices.iter().map(ToString::to_string).collect(),
            a.write_index().map(str::to_owned),
            a.data_stream,
        )),
        _ => None,
    }
}

fn exists(r: &Rig, name: &str) -> bool {
    r.descriptors
        .get(name, &mut vec![0u8; 1 << 16])
        .unwrap()
        .is_some()
}

/// `logs` in two aliases before the take: `shared`, which it writes, and
/// `plain`.
fn with_aliases(r: &Rig) {
    let now = || 1_700_000_000_000i64;
    let pause = |_: Duration| {};
    let mut m = AliasMutations::new(
        &r.descriptors,
        AliasClock {
            now_millis: &now,
            pause: &pause,
        },
        30_000,
        1 << 16,
    );
    let mut shared = AliasRecord::new("shared", &["logs"]);
    shared.write_member = Some("logs".into());
    m.create_alias(&shared).unwrap();
    m.create_alias(&AliasRecord::new("plain", &["logs"]))
        .unwrap();
}

fn eventually(what: &str, mut ok: impl FnMut() -> bool) {
    let start = Instant::now();
    while !ok() {
        assert!(start.elapsed() < Duration::from_secs(10), "{what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_restore_adds_the_recorded_aliases_and_a_write_target_keeps_its_role() {
    let r = taken_with_aliases(true, true, with_aliases);
    let rename = r#","include_aliases":true,"rename_alias_pattern":"^plain$","rename_alias_replacement":"plain2""#;
    let reply = send(&r, "POST", RESTORE, Some(&body(rename)));
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert!(
        reply.text().contains(r#""aliases_restored":2"#),
        "{}",
        reply.text()
    );
    assert!(!reply.text().contains("failed_aliases"), "{}", reply.text());
    // `shared` gained the copy and logs keeps writing; `plain2` is new.
    let shared = alias_of(&r, "shared").unwrap();
    assert_eq!(shared.0, ["logs", "copy"]);
    assert_eq!(shared.1.as_deref(), Some("logs"));
    let plain2 = alias_of(&r, "plain2").unwrap();
    assert_eq!(plain2.0, ["copy"]);
    // The original alias is untouched.
    assert_eq!(alias_of(&r, "plain").unwrap().0, ["logs"]);
}

#[test]
fn without_include_aliases_no_alias_changes_and_the_answer_says_nothing_of_them() {
    let r = taken_with_aliases(true, true, with_aliases);
    let reply = send(&r, "POST", RESTORE, Some(&body("")));
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert!(!reply.text().contains("aliases"), "{}", reply.text());
    assert_eq!(alias_of(&r, "shared").unwrap().0, ["logs"]);
}

#[test]
fn a_snapshot_taken_without_capture_restores_no_aliases_and_says_so() {
    let r = taken_with_aliases(true, false, with_aliases);
    let reply = send(
        &r,
        "POST",
        RESTORE,
        Some(&body(r#","include_aliases":true"#)),
    );
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert!(
        reply.text().contains(r#""aliases_restored":0"#),
        "{}",
        reply.text()
    );
    assert_eq!(alias_of(&r, "shared").unwrap().0, ["logs"]);
}

#[test]
fn an_alias_named_like_an_index_is_refused_before_anything_is_written() {
    let r = taken_with_aliases(true, true, with_aliases);
    let before = requests(&r);
    let rename = r#","include_aliases":true,"rename_alias_pattern":"^plain$","rename_alias_replacement":"logs""#;
    let reply = send(&r, "POST", RESTORE, Some(&body(rename)));
    assert_eq!(reply.status, 400, "{}", reply.text());
    assert!(
        reply.text().contains("invalid_alias_name_exception"),
        "{}",
        reply.text()
    );
    assert!(!exists(&r, "copy"), "nothing was created");
    let puts = requests(&r) - before;
    assert!(puts < 16, "the refusal reads, it does not write: {puts}");
}

#[test]
fn an_alias_named_like_a_data_stream_is_refused_before_the_first_write() {
    let r = taken_with_aliases(true, true, with_aliases);
    let now = || 1_700_000_000_000i64;
    let pause = |_: Duration| {};
    let mut m = AliasMutations::new(
        &r.descriptors,
        AliasClock {
            now_millis: &now,
            pause: &pause,
        },
        30_000,
        1 << 16,
    );
    let at = m.resolve_for_mutation("plain").unwrap().unwrap();
    m.delete_alias(&at.alias, at.generation).unwrap();
    // A data stream now holds the name `shared`.
    let at = m.resolve_for_mutation("shared").unwrap().unwrap();
    m.delete_alias(&at.alias, at.generation).unwrap();
    let mut stream = AliasRecord::new("shared", &["logs"]);
    stream.data_stream = true;
    stream.generation = 1;
    m.create_alias(&stream).unwrap();
    let reply = send(
        &r,
        "POST",
        RESTORE,
        Some(&body(r#","include_aliases":true"#)),
    );
    assert_eq!(reply.status, 400, "{}", reply.text());
    assert!(!exists(&r, "copy"), "refused before the first write");
}

#[test]
fn a_background_restore_adds_the_aliases_after_the_copy() {
    let background = Settings {
        enabled: true,
        max: 2,
        grace: Duration::from_millis(5_000),
    };
    let r = rig_aliased(
        true,
        "",
        64 << 20,
        64,
        StockRestoreSettings::off(),
        background,
        true,
    );
    let _ = rig_full; // the plain rig is the one the other tests use
    crate::snapshot_restore::seed_with(&r, with_aliases);
    let reply = send(
        &r,
        "POST",
        "/_snapshot/backups/daily/_restore",
        Some(&body(r#","include_aliases":true"#)),
    );
    assert_eq!(reply.text(), r#"{"accepted":true}"#);
    eventually("the alias lands after the copy", || {
        alias_of(&r, "shared").is_some_and(|a| a.0.len() == 2)
    });
    eventually("the thread ends", || r.restores.running() == 0);
    assert_eq!(r.restores.failed(), 0);
    assert_eq!(alias_of(&r, "plain").unwrap().0, ["logs", "copy"]);
}

#[test]
fn a_restore_with_aliases_costs_what_the_budget_says() {
    let r = taken_with_aliases(true, true, with_aliases);
    let before = requests(&r);
    let rename = r#","include_aliases":true,"rename_alias_pattern":"^plain$","rename_alias_replacement":"plain2""#;
    let reply = send(&r, "POST", RESTORE, Some(&body(rename)));
    assert_eq!(reply.status, 200, "{}", reply.text());
    budget::check(
        "snapshot.restore_aliases.requests",
        requests(&r) - before,
        "a restore of one index of two shards into two aliases",
    );
}

// AC-2: the take reads each distinct alias once, and nothing more.
#[test]
fn a_take_with_aliases_costs_one_read_for_each_distinct_alias() {
    let cost = |capture: bool| {
        let r = rig_aliased(
            true,
            "",
            64 << 20,
            64,
            StockRestoreSettings::off(),
            Settings::off(),
            capture,
        );
        let body =
            r#"{"type":"fs","settings":{"location":"x","remote_store_index_shallow_copy":true}}"#;
        assert_eq!(
            send(&r, "PUT", "/_snapshot/backups", Some(body)).status,
            200
        );
        crate::snapshot_restore::live_index(&r, "logs", "u1", 2, b"payload");
        with_aliases(&r);
        let before = requests(&r);
        let made = send(
            &r,
            "PUT",
            "/_snapshot/backups/daily?wait_for_completion=true",
            Some(crate::snapshot_take::TAKE),
        );
        assert_eq!(made.status, 200, "{}", made.text());
        requests(&r) - before
    };
    let (off, on) = (cost(false), cost(true));
    assert_eq!(on - off, 2, "two distinct aliases, one read each");
    budget::check(
        "snapshot.take_aliases.requests",
        on,
        "a take of one index of two shards in two aliases",
    );
}

// AC-8: an alias that cannot be added after the copy is named in the answer
// and the restored index stays.
#[test]
fn an_alias_another_writer_holds_is_named_and_the_restored_index_stays() {
    let r = taken_with_aliases(true, true, with_aliases);
    // A mutation of `shared` another writer started and has not finished, with
    // a deadline far ahead, so this restore can neither recover nor wait it out.
    let mut buf = vec![0u8; 1 << 16];
    let held = r.descriptors.resolve_raw("shared", &mut buf).unwrap();
    let Target::Alias(alias) = held.target else {
        panic!("an alias");
    };
    let at = (held.generation, alias.into_owned());
    let pending = AliasRecord {
        mutation: Some(oss_format::AliasMutation {
            until: 1_700_000_000_000 + 10_000_000_000,
            creating: false,
            additions: Vec::new(),
        }),
        ..at.1.clone()
    };
    r.descriptors
        .update_alias(&pending, at.0, &mut vec![0u8; 1 << 16])
        .unwrap()
        .unwrap();
    let rename = r#","include_aliases":true,"rename_alias_pattern":"^plain$","rename_alias_replacement":"plain2""#;
    let reply = send(&r, "POST", RESTORE, Some(&body(rename)));
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert!(
        reply.text().contains(r#""failed_aliases":["shared"]"#),
        "{}",
        reply.text()
    );
    assert!(
        reply.text().contains(r#""aliases_restored":1"#),
        "{}",
        reply.text()
    );
    assert!(exists(&r, "copy"), "the index is whole");
    assert_eq!(alias_of(&r, "plain2").unwrap().0, ["copy"]);
}
