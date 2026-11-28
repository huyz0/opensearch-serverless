//! docs/specs/alerting/SPEC.md, slice 2: the monitor index format.

use oss_format::{
    FormatError, MAX_MONITOR_ID_BYTES, MAX_MONITOR_INDEX_BYTES, MAX_MONITORS, MAX_SCHEDULE_BYTES,
    MonitorEntry, MonitorIndex, monitor_content_key, monitor_index_key,
};

fn entry(content: &str, enabled: bool) -> MonitorEntry {
    MonitorEntry {
        content: content.to_owned(),
        enabled,
        schedule: "period:1:MINUTES".to_owned(),
        updated_ms: 1_700_000_000_000,
        version: 3,
        seq_no: 17,
    }
}

fn written(index: &MonitorIndex) -> String {
    let mut buf = vec![0u8; MAX_MONITOR_INDEX_BYTES];
    let n = index.write(&mut buf).unwrap();
    buf.truncate(n);
    String::from_utf8(buf).unwrap()
}

#[test]
fn an_index_is_written_in_id_order_and_reads_back_the_same() {
    let mut index = MonitorIndex::new(3);
    index.insert("zeta", entry("c-z", true)).unwrap();
    index.insert("alpha", entry("c-a", false)).unwrap();
    assert_eq!(
        written(&index),
        r#"{"version":3,"monitors":{"alpha":{"c":"c-a","e":false,"s":"period:1:MINUTES","u":1700000000000,"v":3,"q":17},"zeta":{"c":"c-z","e":true,"s":"period:1:MINUTES","u":1700000000000,"v":3,"q":17}}}"#
    );
    let back = MonitorIndex::parse(written(&index).as_bytes()).unwrap();
    assert_eq!(back, index);
    assert_eq!(back.len(), 2);
    assert!(!back.is_empty());
    assert!(back.any_enabled());
    assert_eq!(back.get("zeta").unwrap().content, "c-z");
    assert!(back.get("beta").is_none());
    assert_eq!(
        back.iter().map(|(id, _)| id).collect::<Vec<_>>(),
        ["alpha", "zeta"]
    );
}

#[test]
fn insert_replaces_remove_removes_and_shares_sees_a_blob_another_monitor_reads() {
    let mut index = MonitorIndex::new(0);
    assert!(index.is_empty() && !index.any_enabled());
    assert!(index.insert("a", entry("c1", false)).unwrap().is_none());
    assert!(!index.any_enabled());
    let old = index.insert("a", entry("c2", true)).unwrap().unwrap();
    assert_eq!(old.content, "c1");
    assert!(index.any_enabled());
    index.insert("b", entry("c2", false)).unwrap();
    assert!(index.shares("c2") && !index.shares("c1"));
    assert_eq!(index.remove("a").unwrap().content, "c2");
    assert!(index.remove("a").is_none());
    assert!(!index.any_enabled());
}

#[test]
fn the_monitor_count_is_bounded_but_a_present_id_may_be_replaced() {
    let mut index = MonitorIndex::new(0);
    for i in 0..MAX_MONITORS {
        index.insert(&format!("m{i:04}"), entry("c", true)).unwrap();
    }
    assert!(matches!(
        index.insert("one-more", entry("c", true)),
        Err(FormatError::TooLarge { .. })
    ));
    index.insert("m0007", entry("c2", false)).unwrap();
    assert_eq!(index.len(), MAX_MONITORS);
    let mut buf = vec![0u8; MAX_MONITOR_INDEX_BYTES];
    let n = index.write(&mut buf).unwrap();
    assert_eq!(MonitorIndex::parse(&buf[..n]).unwrap(), index);
}

#[test]
fn ids_schedules_and_content_keys_are_checked_on_the_way_in() {
    let mut index = MonitorIndex::new(0);
    for bad in ["", ".", "..", "a/b", "a\nb"] {
        assert!(index.insert(bad, entry("c", true)).is_err(), "{bad:?}");
    }
    assert!(matches!(
        index.insert(&"x".repeat(MAX_MONITOR_ID_BYTES + 1), entry("c", true)),
        Err(FormatError::TooLarge { .. })
    ));
    index
        .insert(&"x".repeat(MAX_MONITOR_ID_BYTES), entry("c", true))
        .unwrap();
    let mut long = entry("c", true);
    long.schedule = "s".repeat(MAX_SCHEDULE_BYTES + 1);
    assert!(matches!(
        index.insert("m", long),
        Err(FormatError::TooLarge { .. })
    ));
    let mut max = entry("c", true);
    max.schedule = "s".repeat(MAX_SCHEDULE_BYTES);
    index.insert("m", max).unwrap();
    for bad in ["", ".", "..", "a/b"] {
        assert!(index.insert("n", entry(bad, true)).is_err(), "{bad:?}");
    }
}

#[test]
fn a_damaged_index_is_refused_never_taken() {
    let good = r#"{"version":1,"monitors":{"a":{"c":"x","e":true,"s":"p","u":1,"v":1,"q":1}}}"#;
    assert!(MonitorIndex::parse(good.as_bytes()).is_ok());
    for bad in [
        "",
        "{}",
        r#"{"version":1}"#,
        r#"{"monitors":{}}"#,
        r#"{"version":1,"version":2,"monitors":{}}"#,
        r#"{"version":1,"monitors":{},"monitors":{}}"#,
        r#"{"version":"1","monitors":{}}"#,
        r#"{"version":1,"monitors":{},"extra":1}"#,
        r#"{"version":1,"monitors":{"a":{"c":"x","e":true,"s":"p"}}}"#,
        r#"{"version":1,"monitors":{"a":{"c":"x","c":"y","e":true,"s":"p","u":1,"v":1,"q":1}}}"#,
        r#"{"version":1,"monitors":{"a":{"c":"x","e":true,"e":false,"s":"p","u":1,"v":1,"q":1}}}"#,
        r#"{"version":1,"monitors":{"a":{"c":"x","e":true,"s":"p","s":"q","u":1,"v":1,"q":1}}}"#,
        r#"{"version":1,"monitors":{"a":{"c":"x","e":true,"s":"p","u":1,"u":2,"v":1,"q":1}}}"#,
        r#"{"version":1,"monitors":{"a":{"c":"x","e":"yes","s":"p","u":1,"v":1,"q":1}}}"#,
        r#"{"version":1,"monitors":{"a":{"c":"a/b","e":true,"s":"p","u":1,"v":1,"q":1}}}"#,
        r#"{"version":1,"monitors":{"a":{"c":"x","e":true,"s":"p","u":1,"v":1,"q":1},"a":{"c":"x","e":true,"s":"p","u":1,"v":1,"q":1}}}"#,
        r#"{"version":1,"monitors":{"":{"c":"x","e":true,"s":"p","u":1,"v":1,"q":1}}}"#,
        r#"{"version":1,"monitors":{"a":{"c":"x","e":true,"s":"p","u":1,"q":1}}}"#,
        r#"{"version":1,"monitors":{"a":{"c":"x","e":true,"s":"p","u":1,"v":1}}}"#,
        r#"{"version":1,"monitors":{"a":{"c":"x","e":true,"s":"p","u":1,"v":1,"v":2,"q":1}}}"#,
        r#"{"version":1,"monitors":{"a":{"c":"x","e":true,"s":"p","u":1,"v":1,"q":1,"q":2}}}"#,
    ] {
        assert!(MonitorIndex::parse(bad.as_bytes()).is_err(), "{bad}");
    }
    assert!(matches!(
        MonitorIndex::parse(&vec![b' '; MAX_MONITOR_INDEX_BYTES + 1]),
        Err(FormatError::TooLarge { .. })
    ));
}

#[test]
fn a_write_into_a_short_buffer_is_refused() {
    let mut index = MonitorIndex::new(1);
    index.insert("a", entry("c", true)).unwrap();
    let need = written(&index).len();
    assert!(matches!(
        index.write(&mut vec![0u8; need - 1]),
        Err(FormatError::TooLarge { .. })
    ));
    assert_eq!(index.write(&mut vec![0u8; need]).unwrap(), need);
}

#[test]
fn the_largest_index_is_taken_and_one_byte_more_is_refused() {
    assert_eq!(oss_format::MAX_MONITOR_BYTES, 1_048_576);
    let good = br#"{"version":1,"monitors":{}}"#;
    let mut max = good.to_vec();
    max.resize(MAX_MONITOR_INDEX_BYTES, b' ');
    assert!(MonitorIndex::parse(&max).is_ok());
    max.push(b' ');
    assert!(matches!(
        MonitorIndex::parse(&max),
        Err(FormatError::TooLarge { .. })
    ));
}

#[test]
fn keys_are_derived_from_the_base_and_the_tenant() {
    assert_eq!(
        monitor_index_key("d/", "acme").unwrap(),
        "d/alerting/acme/monitors/_index"
    );
    assert_eq!(
        monitor_content_key("", "acme", "k1").unwrap(),
        "alerting/acme/monitors/blob-k1"
    );
    assert!(monitor_index_key("d", "acme").is_err());
    assert!(monitor_index_key("", "a/b").is_err());
    assert!(monitor_content_key("", "acme", "a/b").is_err());
    assert!(monitor_content_key("", "acme", "").is_err());
    assert!(monitor_content_key("", "acme", ".").is_err());
    assert!(monitor_content_key("", "acme", "..").is_err());
}

#[test]
fn a_quote_backslash_or_control_character_in_an_id_or_schedule_is_refused() {
    for bad in ["a\"b", "a\\b", "a\nb"] {
        let mut index = MonitorIndex::new(0);
        let entry = |schedule: &str| MonitorEntry {
            content: "c".to_owned(),
            enabled: true,
            schedule: schedule.to_owned(),
            updated_ms: 0,
            version: 1,
            seq_no: 1,
        };
        assert!(index.insert(bad, entry("p")).is_err(), "id {bad:?}");
        assert!(index.insert("m", entry(bad)).is_err(), "schedule {bad:?}");
    }
}

#[test]
fn a_full_index_of_the_longest_entries_fits_its_bound() {
    let mut index = MonitorIndex::new(i64::MAX);
    for i in 0..oss_format::MAX_MONITORS {
        let id = format!("{i:0>255}");
        index
            .insert(
                &id,
                MonitorEntry {
                    content: "0123456789abcdef0123456789abcdef".to_owned(),
                    enabled: true,
                    schedule: "s".repeat(oss_format::MAX_SCHEDULE_BYTES),
                    updated_ms: i64::MIN,
                    version: i64::MIN,
                    seq_no: i64::MIN,
                },
            )
            .unwrap();
    }
    let mut buf = vec![0u8; oss_format::MAX_MONITOR_INDEX_BYTES];
    let n = index.write(&mut buf).unwrap();
    assert_eq!(
        MonitorIndex::parse(&buf[..n]).unwrap().len(),
        oss_format::MAX_MONITORS
    );
}
