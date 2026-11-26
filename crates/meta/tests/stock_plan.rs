//! docs/specs/stock-snapshot-restore AC-6 and AC-9: a restore reads the
//! four rounds and refuses what it cannot carry before any write; the
//! metadata of an index nobody asked for is never read.

use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, ReadOnlyStore, S3Op, SliceSource};
use oss_meta::{
    StockLimits, StockPlanError, StockReadError, StockRepo, StockRules, stock_load, stock_plan,
};

const PREFIX: &str = "src/repo/";
const SNAP: &str = "Aa11Bb22Cc33Dd44Ee55Ff";
const LOGS: &str = "Zq1_aBcD-EfGhIjKlMnOpQ";
const USERS: &str = "u2ZyXwVuTsRqPoNmLkJiHg";

fn fixture(dir: &str, name: &str) -> Vec<u8> {
    let path = format!(
        "{}/../../fixtures/snapshots/{dir}/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    #[allow(clippy::disallowed_methods)] // ARITH: a test reads a recorded fixture.
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    bytes
}

fn put(store: &dyn BlobStore, key: &str, bytes: &[u8]) {
    store
        .put(
            &format!("{PREFIX}{key}"),
            &mut SliceSource::new(bytes),
            &mut [0u8; 64],
        )
        .expect("put");
}

/// A repository of two indices, `logs` and `users`, each of two shards;
/// `users`' metadata is there only when `with_users`.
fn world(with_users: bool) -> (Arc<Emulator>, ReadOnlyStore) {
    let inner = Arc::new(Emulator::new(64 << 20, 1));
    put(&*inner, "index.latest", &7i64.to_be_bytes());
    put(
        &*inner,
        "index-7",
        &fixture("stock-repository-data", "index-7.json"),
    );
    put(
        &*inner,
        &format!("snap-{SNAP}.dat"),
        &fixture("stock-metadata", "snapshot-info.ok.dat"),
    );
    let meta = fixture("stock-metadata", "index-metadata.plain.dat");
    let shard = fixture("stock-metadata", "snapshot.deflate.dat");
    let mut indices = vec![(LOGS, "idLogsAaaa")];
    if with_users {
        indices.push((USERS, "idUsersBbbb"));
    }
    for (id, meta_id) in indices {
        put(&*inner, &format!("indices/{id}/meta-{meta_id}.dat"), &meta);
        for n in 0..2 {
            put(
                &*inner,
                &format!("indices/{id}/{n}/snap-{SNAP}.dat"),
                &shard,
            );
        }
    }
    let ro = ReadOnlyStore::new(inner.clone());
    (inner, ro)
}

fn rules<'a>(
    keeps: &'a dyn Fn(&str) -> bool,
    mapping_ok: &'a dyn Fn(&str) -> bool,
) -> StockRules<'a> {
    StockRules {
        limits: StockLimits::default(),
        fanout: 4,
        keeps,
        mapping_ok,
    }
}

fn run(
    ro: &ReadOnlyStore,
    wanted: Option<&[String]>,
    rules: &StockRules<'_>,
) -> Result<oss_meta::StockPlan, StockPlanError> {
    let repo = StockRepo::new(ro, PREFIX);
    let data = stock_load(&repo).expect("load");
    stock_plan(&repo, &data, "snap-1", wanted, rules)
}

fn all(_: &str) -> bool {
    true
}

#[test]
fn one_requested_index_reads_only_its_own_blobs() {
    // `users` has no metadata blob at all: reading it would fail.
    let (inner, ro) = world(false);
    let plan = run(&ro, Some(&["logs".to_owned()]), &rules(&all, &all)).expect("plan");
    assert_eq!(plan.uuid, SNAP);
    assert_eq!(plan.indices.len(), 1);
    let logs = &plan.indices[0];
    assert_eq!((logs.name.as_str(), logs.id.as_str()), ("logs", LOGS));
    assert_eq!(logs.native.number_of_shards, 2);
    assert_eq!(logs.shards.len(), 2);
    // index.latest, index-N, the snapshot, one metadata, two shards.
    assert_eq!(inner.meter().count(S3Op::Get), 6);
    assert_eq!(inner.meter().count(S3Op::List), 0);
}

#[test]
fn every_index_when_none_is_named() {
    let (inner, ro) = world(true);
    let plan = run(&ro, None, &rules(&all, &all)).expect("plan");
    let names: Vec<_> = plan.indices.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["logs", "users"]);
    assert!(plan.indices.iter().all(|i| i.shards.len() == 2));
    assert_eq!(inner.meter().count(S3Op::Get), 9);
    for width in [1, 2, 64] {
        let mut r = rules(&all, &all);
        r.fanout = width;
        assert_eq!(run(&ro, None, &r).expect("plan").indices.len(), 2);
    }
}

#[test]
fn an_unknown_snapshot_or_index_is_named() {
    let (_, ro) = world(true);
    let repo = StockRepo::new(&ro, PREFIX);
    let data = stock_load(&repo).expect("load");
    let r = rules(&all, &all);
    assert!(matches!(
        stock_plan(&repo, &data, "nope", None, &r),
        Err(StockPlanError::UnknownSnapshot(n)) if n == "nope"
    ));
    // `snap-2` holds `logs` only.
    assert!(matches!(
        stock_plan(&repo, &data, "snap-2", Some(&["users".to_owned()]), &r),
        Err(StockPlanError::UnknownIndex(n)) if n == "users"
    ));
}

#[test]
fn a_snapshot_the_restore_cannot_carry_is_refused_before_any_shard_is_read() {
    for blob in [
        "snapshot-info.global.dat",
        "snapshot-info.partial.dat",
        "snapshot-info.shallow.dat",
    ] {
        let (inner, ro) = world(true);
        put(
            &*inner,
            &format!("snap-{SNAP}.dat"),
            &fixture("stock-metadata", blob),
        );
        let before = inner.meter().count(S3Op::Get);
        let err = run(&ro, None, &rules(&all, &all)).unwrap_err();
        assert!(
            matches!(err, StockPlanError::Unsupported(_)),
            "{blob}: {err}"
        );
        // Two for the load, the snapshot and two metadata: no shard blob.
        assert_eq!(inner.meter().count(S3Op::Get) - before, 5, "{blob}");
    }
}

#[test]
fn a_setting_or_mapping_the_node_cannot_keep_is_refused_naming_it() {
    let (_, ro) = world(true);
    let no_settings = |_: &str| false;
    match run(&ro, None, &rules(&no_settings, &all)).unwrap_err() {
        StockPlanError::Unsupported(why) => assert!(why.contains("[index."), "{why}"),
        other => panic!("{other}"),
    }
    let no_mapping = |_: &str| false;
    match run(&ro, None, &rules(&all, &no_mapping)).unwrap_err() {
        StockPlanError::Unsupported(why) => assert!(why.contains("mapping"), "{why}"),
        other => panic!("{other}"),
    }
}

#[test]
fn another_shard_layout_is_refused() {
    let (inner, ro) = world(true);
    let json = String::from_utf8(fixture("stock-repository-data", "index-7.json")).unwrap();
    let json = json.replacen("\"shard_path_type\":0", "\"shard_path_type\":1", 1);
    put(&*inner, "index-7", json.as_bytes());
    assert!(matches!(
        run(&ro, None, &rules(&all, &all)),
        Err(StockPlanError::Unsupported(_))
    ));
}

#[test]
fn each_bound_holds_at_its_count_and_refuses_one_past() {
    let (_, ro) = world(true);
    let cases: [(StockLimits, bool); 6] = [
        (
            StockLimits {
                indices: 2,
                ..StockLimits::default()
            },
            true,
        ),
        (
            StockLimits {
                indices: 1,
                ..StockLimits::default()
            },
            false,
        ),
        (
            StockLimits {
                shards: 4,
                ..StockLimits::default()
            },
            true,
        ),
        (
            StockLimits {
                shards: 3,
                ..StockLimits::default()
            },
            false,
        ),
        (
            StockLimits {
                file_entries: 8,
                ..StockLimits::default()
            },
            true,
        ),
        (
            StockLimits {
                file_entries: 7,
                ..StockLimits::default()
            },
            false,
        ),
    ];
    for (limits, ok) in cases {
        let mut r = rules(&all, &all);
        r.limits = limits;
        let got = run(&ro, None, &r);
        if ok {
            assert!(got.is_ok(), "{limits:?}");
        } else {
            assert!(
                matches!(got, Err(StockPlanError::TooLarge(_))),
                "{limits:?}"
            );
        }
    }
}

#[test]
fn a_missing_blob_fails_the_plan_naming_it() {
    // `users` is requested but has no metadata blob.
    let (_, ro) = world(false);
    match run(&ro, None, &rules(&all, &all)).unwrap_err() {
        StockPlanError::Read(StockReadError::Missing(key)) => {
            assert!(key.contains("meta-idUsersBbbb"), "{key}");
        }
        other => panic!("{other}"),
    }
}

#[test]
fn every_plan_error_says_what_it_is() {
    let errors = [
        StockPlanError::UnknownSnapshot("s1".into()),
        StockPlanError::UnknownIndex("i1".into()),
        StockPlanError::Unsupported("why1".into()),
        StockPlanError::TooLarge("things"),
        StockPlanError::Corrupt("oops"),
        StockPlanError::Read(StockReadError::Missing("k1".into())),
    ];
    let texts: Vec<String> = errors.iter().map(ToString::to_string).collect();
    for (t, needle) in texts
        .iter()
        .zip(["s1", "i1", "why1", "things", "oops", "k1"])
    {
        assert!(t.contains(needle), "{t}");
    }
}
