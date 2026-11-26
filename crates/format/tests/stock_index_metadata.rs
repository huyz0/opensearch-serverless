//! docs/specs/stock-snapshot-restore AC-4 and AC-6: the index metadata blob
//! the server's own `INDEX_METADATA_FORMAT` wrote
//! (`fixtures/snapshots/stock-metadata/GenMetadata.java`) reads, plain and
//! deflated, to the settings, mapping and aliases a restore uses; a hostile
//! one is refused.

use oss_format::{
    FormatError, STOCK_MAX_ALIASES, STOCK_MAX_SETTINGS, StockEnvelopeLimits, StockIndexMetadata,
    StockRefusal, stock_blob_to_json,
};

#[allow(clippy::disallowed_methods)] // ARITH: a test reads a recorded fixture.
fn json(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/../../fixtures/snapshots/stock-metadata/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let blob = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut out = Vec::new();
    stock_blob_to_json(
        &blob,
        "index-metadata",
        &StockEnvelopeLimits::default(),
        &mut out,
    )
    .expect("decodes");
    out
}

fn parse(text: &str) -> Result<StockIndexMetadata, FormatError> {
    StockIndexMetadata::parse(text.as_bytes())
}

#[test]
fn the_recorded_blobs_read_plain_and_deflated() {
    let plain = StockIndexMetadata::parse(&json("index-metadata.plain.dat")).expect("plain");
    let deflate = StockIndexMetadata::parse(&json("index-metadata.deflate.dat")).expect("deflate");
    assert_eq!(plain.name, "logs");
    assert_eq!(plain.name, deflate.name);
    assert_eq!(plain.state, "open");
    assert_eq!(plain.setting("index.number_of_shards"), Some("2"));
    assert_eq!(plain.setting("index.no_such_key"), None);
    assert_eq!(plain.aliases, vec!["logs-alias".to_string()]);
    let mapping = plain.mapping.as_deref().expect("a mapping");
    assert!(mapping.contains("\"properties\""), "{mapping}");
    assert_eq!(plain.settings, deflate.settings);
    assert_eq!(plain.mapping, deflate.mapping);
}

#[test]
fn every_prefix_of_the_recorded_json_is_refused() {
    let full = json("index-metadata.plain.dat");
    for n in 0..full.len() {
        assert!(StockIndexMetadata::parse(&full[..n]).is_err(), "prefix {n}");
    }
}

#[test]
fn a_shape_the_java_does_not_write_is_refused() {
    for text in [
        "[]",
        "{}",
        r#"{"a":{}}"#,
        r#"{"a":{"state":3}}"#,
        r#"{"a":{"state":"open"},"b":{"state":"open"}}"#,
        r#"{"a":{"state":"open","settings":[]}}"#,
        r#"{"a":{"state":"open","settings":{"k":1}}}"#,
        r#"{"a":{"state":"open"}} x"#,
    ] {
        assert!(parse(text).is_err(), "{text}");
    }
}

#[test]
fn what_a_restore_does_not_carry_is_not_followable() {
    for text in [
        r#"{"a":{"state":"open","settings":{"k":["x"]}}}"#,
        r#"{"a":{"state":"open","mappings":[{"other":{}}]}}"#,
    ] {
        assert!(
            matches!(parse(text), Err(FormatError::NotFollowable { .. })),
            "{text}"
        );
    }
}

#[test]
fn an_index_without_mappings_or_aliases_reads() {
    let m = parse(r#"{"a":{"state":"close","version":1}}"#).expect("reads");
    assert_eq!(m.state, "close");
    assert!(m.mapping.is_none() && m.aliases.is_empty() && m.settings.is_empty());
}

#[test]
fn the_limits_hold_at_their_count_and_one_past() {
    let settings = |n: usize| {
        let body: Vec<String> = (0..n).map(|i| format!(r#""k{i}":"v""#)).collect();
        format!(
            r#"{{"a":{{"state":"open","settings":{{{}}}}}}}"#,
            body.join(",")
        )
    };
    assert_eq!(
        parse(&settings(STOCK_MAX_SETTINGS))
            .expect("at")
            .settings
            .len(),
        STOCK_MAX_SETTINGS
    );
    assert!(matches!(
        parse(&settings(STOCK_MAX_SETTINGS + 1)),
        Err(FormatError::TooLarge { .. })
    ));
    let aliases = |n: usize| {
        let body: Vec<String> = (0..n).map(|i| format!(r#""a{i}":{{}}"#)).collect();
        format!(
            r#"{{"a":{{"state":"open","aliases":{{{}}}}}}}"#,
            body.join(",")
        )
    };
    assert_eq!(
        parse(&aliases(STOCK_MAX_ALIASES))
            .expect("at")
            .aliases
            .len(),
        STOCK_MAX_ALIASES
    );
    assert!(matches!(
        parse(&aliases(STOCK_MAX_ALIASES + 1)),
        Err(FormatError::TooLarge { .. })
    ));
}

fn keeps(key: &str) -> bool {
    key == "index.refresh_interval"
}

fn with(settings: &str) -> StockIndexMetadata {
    parse(&format!(
        r#"{{"a":{{"state":"open","settings":{{{settings}}}}}}}"#
    ))
    .expect("reads")
}

#[test]
fn the_recorded_index_becomes_a_native_one() {
    let m = StockIndexMetadata::parse(&json("index-metadata.plain.dat")).expect("reads");
    let n = m
        .native(&|k| k == "index.refresh_interval")
        .expect("native");
    assert_eq!(n.number_of_shards, 2);
    assert_eq!(n.mapping, m.mapping);
    // Only what the node keeps is left: identity and replicas are dropped.
    let text = String::from_utf8(n.settings).expect("utf8");
    assert!(text.starts_with('{') && text.ends_with('}'), "{text}");
    assert!(
        !text.contains("uuid") && !text.contains("replicas"),
        "{text}"
    );
}

#[test]
fn settings_are_kept_dropped_or_refused() {
    let m = with(
        r#""index.number_of_shards":"3","index.uuid":"u","index.refresh_interval":"5s","index.number_of_replicas":"1""#,
    );
    let n = m.native(&keeps).expect("native");
    assert_eq!(n.number_of_shards, 3);
    assert_eq!(n.settings, br#"{"index.refresh_interval":"5s"}"#);
    let m = with(r#""index.number_of_shards":"1","index.odd":"x""#);
    assert_eq!(
        m.native(&keeps),
        Err(StockRefusal::Setting("index.odd".into()))
    );
}

#[test]
fn a_shard_count_and_a_partition_size_are_checked() {
    for count in [
        r#""index.number_of_shards":"0""#,
        r#""index.number_of_shards":"x""#,
        "",
    ] {
        assert_eq!(with(count).native(&keeps), Err(StockRefusal::NoShardCount));
    }
    let size = |n: &str| {
        with(&format!(
            r#""index.number_of_shards":"1","index.routing_partition_size":"{n}""#
        ))
        .native(&keeps)
    };
    assert!(size("1").is_ok());
    assert_eq!(size("2"), Err(StockRefusal::PartitionSize));
    assert_eq!(size("x"), Err(StockRefusal::PartitionSize));
    assert!(!StockRefusal::PartitionSize.to_string().is_empty());
}

#[test]
fn several_kept_settings_are_joined_in_order() {
    let m =
        with(r#""index.number_of_shards":"1","index.refresh_interval":"5s","index.codec":"zstd""#);
    let n = m
        .native(&|k| k == "index.refresh_interval" || k == "index.codec")
        .expect("native");
    assert_eq!(
        n.settings,
        br#"{"index.refresh_interval":"5s","index.codec":"zstd"}"#
    );
}

/// A 3.10 node writes `index.replication.type: DOCUMENT` on every index it
/// creates (recorded in `fixtures/snapshots/stock`): it names the default,
/// which a one-copy index has, so a restore drops it. Segment replication
/// is not the default and stays refused.
#[test]
fn the_default_replication_type_is_dropped_and_another_is_refused() {
    let m = with(
        r#""index.number_of_shards":"1","index.replication.type":"DOCUMENT","index.refresh_interval":"5s""#,
    );
    let n = m.native(&keeps).expect("native");
    assert_eq!(n.settings, br#"{"index.refresh_interval":"5s"}"#);
    let m = with(r#""index.number_of_shards":"1","index.replication.type":"SEGMENT""#);
    assert_eq!(
        m.native(&keeps),
        Err(StockRefusal::Setting("index.replication.type".into()))
    );
}
