//! docs/specs/stock-snapshot-restore AC-4 and AC-6: a snapshot's own blob,
//! written by the server's `SNAPSHOT_FORMAT`
//! (`fixtures/snapshots/stock-metadata/GenSnapshotInfo.java`), reads to what a
//! restore checks; a partial, global-state, shallow or unsupported-version
//! snapshot is refused before anything is written.

use oss_format::{
    FormatError, STOCK_MAX_INDICES, StockEnvelopeLimits, StockSnapshotInfo, stock_blob_to_json,
};

#[allow(clippy::disallowed_methods)] // ARITH: a test reads a recorded fixture.
fn json(stem: &str) -> Vec<u8> {
    let path = format!(
        "{}/../../fixtures/snapshots/stock-metadata/snapshot-info.{stem}.dat",
        env!("CARGO_MANIFEST_DIR")
    );
    let blob = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut out = Vec::new();
    stock_blob_to_json(&blob, "snapshot", &StockEnvelopeLimits::default(), &mut out)
        .expect("decodes");
    out
}

fn read(stem: &str) -> StockSnapshotInfo {
    StockSnapshotInfo::parse(&json(stem)).expect("reads")
}

fn parse(text: &str) -> Result<StockSnapshotInfo, FormatError> {
    StockSnapshotInfo::parse(text.as_bytes())
}

#[test]
fn a_clean_snapshot_reads_and_is_not_refused() {
    let s = read("ok");
    assert_eq!(
        (s.name.as_str(), s.uuid.as_str()),
        ("snap-1", "SnapUuidAaaaBbbbCccc01")
    );
    assert_eq!(s.state, "SUCCESS");
    assert_eq!(s.indices, ["logs", "metrics"]);
    assert_eq!((s.total_shards, s.successful_shards, s.failures), (3, 3, 0));
    assert_eq!(s.include_global_state, Some(false));
    assert!(!s.remote_store_shallow);
    assert_eq!(s.refusal(), None);
}

#[test]
fn each_snapshot_a_restore_does_not_take_says_why() {
    let why = |stem: &str| {
        read(stem)
            .refusal()
            .unwrap_or_else(|| panic!("{stem} is taken"))
    };
    assert!(why("global").contains("global state"));
    assert!(why("shallow").contains("shallow"));
    let partial = read("partial");
    assert_eq!(
        (
            partial.state.as_str(),
            partial.failures,
            partial.successful_shards
        ),
        ("PARTIAL", 1, 2)
    );
    assert!(why("partial").contains("not complete"));
}

#[test]
fn a_failed_shard_or_a_missing_count_is_refused_even_when_the_state_says_success() {
    let with = |extra: &str| {
        parse(&format!(
            r#"{{"snapshot":{{"name":"a","uuid":"u","state":"SUCCESS","version_id":137317827,"total_shards":2,"successful_shards":2{extra}}}}}"#
        ))
        .expect("reads")
    };
    assert_eq!(with("").refusal(), None);
    assert!(
        with(r#","failures":[{"a":[1,{"b":2}]}]"#)
            .refusal()
            .unwrap()
            .contains("failed")
    );
    let fewer = parse(
        r#"{"snapshot":{"name":"a","uuid":"u","state":"SUCCESS","version_id":137317827,"total_shards":2,"successful_shards":1}}"#,
    )
    .expect("reads");
    assert!(fewer.refusal().unwrap().contains("failed"));
}

#[test]
fn the_version_must_be_a_supported_opensearch_one() {
    let with = |id: i64| {
        parse(&format!(
            r#"{{"snapshot":{{"name":"a","uuid":"u","state":"SUCCESS","version_id":{id},"total_shards":0,"successful_shards":0}}}}"#
        ))
        .expect("reads")
        .refusal()
    };
    let mask = 0x0800_0000;
    assert_eq!(with(3_000_099 + mask), None, "3.0.0");
    assert_eq!(with(3_109_999 + mask), None, "3.10.99");
    for refused in [
        3_000_098 + mask,
        2_190_099 + mask,
        3_110_099 + mask,
        4_000_099 + mask,
        3_100_099,
        7_100_099,
    ] {
        assert!(with(refused).unwrap().contains("version"), "{refused}");
    }
}

#[test]
fn a_hostile_snapshot_blob_is_refused() {
    let full = json("partial");
    for n in 0..full.len() {
        assert!(StockSnapshotInfo::parse(&full[..n]).is_err(), "prefix {n}");
    }
    for text in [
        "[]",
        "{}",
        r#"{"snapshot":3}"#,
        r#"{"snapshot":{"uuid":"u","state":"SUCCESS"}}"#,
        r#"{"snapshot":{"name":"a","uuid":"u"}}"#,
        r#"{"snapshot":{"name":"a","uuid":"u","state":3}}"#,
        r#"{"snapshot":{"name":"a","uuid":"u","state":"SUCCESS","total_shards":-1}}"#,
        r#"{"snapshot":{"name":"a","uuid":"u","state":"SUCCESS","indices":{}}}"#,
        r#"{"snapshot":{"name":"a","uuid":"u","state":"SUCCESS","indices":[1]}}"#,
        r#"{"snapshot":{"name":"a","uuid":"u","state":"SUCCESS","failures":{}}}"#,
        r#"{"snapshot":{"name":"a","uuid":"u","state":"SUCCESS","include_global_state":"yes"}}"#,
        r#"{"snapshot":{"name":"a","uuid":"u","state":"SUCCESS"}} x"#,
    ] {
        assert!(parse(text).is_err(), "{text}");
    }
}

#[test]
fn the_index_list_holds_at_its_count_and_not_past() {
    let with = |n: usize| {
        let names: Vec<String> = (0..n).map(|i| format!(r#""i{i}""#)).collect();
        parse(&format!(
            r#"{{"snapshot":{{"name":"a","uuid":"u","state":"SUCCESS","indices":[{}]}}}}"#,
            names.join(",")
        ))
    };
    assert_eq!(
        with(STOCK_MAX_INDICES).expect("at").indices.len(),
        STOCK_MAX_INDICES
    );
    assert!(matches!(
        with(STOCK_MAX_INDICES + 1),
        Err(FormatError::TooLarge { .. })
    ));
}
