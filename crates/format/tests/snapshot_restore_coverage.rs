//! The restore reader's shard coverage: a finished index names every shard once.

use oss_format::RestoreIndex;

fn shards_of(count: u32, numbers: &[u32]) -> RestoreIndex {
    let shards = numbers
        .iter()
        .map(|n| format!(r#"{{"shard":{n},"term":1,"files":{{}}}}"#))
        .collect::<Vec<_>>()
        .join(",");
    let record = format!(
        r#"{{"repo":"r","name":"s","shallow":true,"start_time_in_millis":1,"end_time_in_millis":2,
            "indices":{{"a":{{"uuid":"U","number_of_shards":{count},"shards":[{shards}]}}}}}}"#
    );
    RestoreIndex::find(record.as_bytes(), "a").unwrap().unwrap()
}

#[test]
fn a_finished_index_must_name_every_shard_once() {
    assert!(shards_of(3, &[2, 0, 1]).refusal().unwrap().is_none());
    assert!(shards_of(3, &[0, 1]).refusal().is_err());
    assert!(shards_of(2, &[0, 0]).refusal().is_err());
    assert!(shards_of(2, &[0, 1, 1]).refusal().is_err());
    assert!(shards_of(2, &[0, 2]).refusal().is_err());
    // The in-progress marker lists nothing and is not a finished index.
    assert!(shards_of(2, &[]).refusal().unwrap().is_none());
}

#[test]
fn the_shard_list_is_bounded_before_it_is_allocated() {
    let all: Vec<u32> = (0..4096).collect();
    assert!(shards_of(4096, &all).refusal().unwrap().is_none());
    let over: Vec<u32> = (0..4097).collect();
    let record = format!(
        r#"{{"repo":"r","name":"s","shallow":true,"start_time_in_millis":1,"end_time_in_millis":2,
            "indices":{{"a":{{"uuid":"U","number_of_shards":4096,"shards":[{}]}}}}}}"#,
        over.iter()
            .map(|n| format!(r#"{{"shard":{n},"term":1,"files":{{}}}}"#))
            .collect::<Vec<_>>()
            .join(",")
    );
    assert!(RestoreIndex::find(record.as_bytes(), "a").is_err());
}
