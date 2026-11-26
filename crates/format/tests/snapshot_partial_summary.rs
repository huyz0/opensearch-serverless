//! The summary parser's reading of `failed_shards`: coverage, order and bounds.

use oss_format::SnapshotSummary;

fn partial_record(count: u32, captured: &[u32], failed: &str) -> String {
    let shards = captured
        .iter()
        .map(|n| format!(r#"{{"shard":{n},"term":1,"files":{{}}}}"#))
        .collect::<Vec<_>>()
        .join(",");
    let failed = if failed.is_empty() {
        String::new()
    } else {
        format!(r#","failed_shards":[{failed}]"#)
    };
    format!(
        r#"{{"repo":"r","name":"s","shallow":false,"start_time_in_millis":1,"end_time_in_millis":2,"indices":{{"a":{{"uuid":"A","number_of_shards":{count},"shards":[{shards}]{failed}}}}}}}"#
    )
}

#[test]
fn a_partial_index_is_captured_and_failed_shards_together() {
    let s = SnapshotSummary::parse(partial_record(4, &[0, 2], "1,3").as_bytes()).unwrap();
    let a = &s.indices[0];
    assert_eq!((a.captured, a.failed_shards.clone()), (2, vec![1, 3]));
    assert!(!a.capturing() && !s.in_progress());
    // Every shard failed: not the in-progress marker.
    let all = SnapshotSummary::parse(partial_record(2, &[], "0,1").as_bytes()).unwrap();
    assert!(!all.indices[0].capturing() && !all.in_progress());
    assert_eq!(all.indices[0].captured, 0);
}

#[test]
fn failed_shards_that_do_not_cover_exactly_are_corrupt() {
    for (count, captured, failed) in [
        (4, vec![0, 2], "1"),
        (4, vec![0, 2], "1,1,3"),
        (4, vec![0, 2], "2,1,3"),
        (4, vec![0, 2], "1,4"),
        (2, vec![0], "-1"),
        (2, vec![0], "\"1\""),
        (2, vec![], "0"),
        // The counts add up; only the shard numbers show the repeat.
        (4, vec![0, 2], "1,1"),
        (3, vec![0, 1], "1"),
        (3, vec![0, 1], "0"),
    ] {
        let record = partial_record(count, &captured, failed);
        assert!(
            SnapshotSummary::parse(record.as_bytes()).is_err(),
            "{record}"
        );
    }
    let not_array = r#"{"repo":"r","name":"s","shallow":false,"start_time_in_millis":1,"end_time_in_millis":2,"indices":{"a":{"uuid":"A","number_of_shards":1,"failed_shards":0}}}"#;
    assert!(SnapshotSummary::parse(not_array.as_bytes()).is_err());
}

#[test]
fn failed_shards_may_come_before_the_captured_ones() {
    let record = r#"{"repo":"r","name":"s","shallow":false,"start_time_in_millis":1,"end_time_in_millis":2,"indices":{"a":{"uuid":"A","number_of_shards":2,"failed_shards":[1],"shards":[{"shard":0,"term":1,"files":{}}]}}}"#;
    assert!(SnapshotSummary::parse(record.as_bytes()).is_ok());
    let twice = record.replace(r#""failed_shards":[1]"#, r#""failed_shards":[0]"#);
    assert!(SnapshotSummary::parse(twice.as_bytes()).is_err());
}

#[test]
fn the_failed_list_is_bounded_before_it_grows() {
    let all: Vec<String> = (0..4096).map(|n| n.to_string()).collect();
    let ok = partial_record(4096, &[], &all.join(","));
    assert!(SnapshotSummary::parse(ok.as_bytes()).is_ok());
    let over: Vec<String> = (0..4097).map(|n| n.to_string()).collect();
    let bad = partial_record(4097, &[], &over.join(","));
    assert!(SnapshotSummary::parse(bad.as_bytes()).is_err());
}
