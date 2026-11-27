//! What each bulk item and the request answer: the refresh a write asks
//! for, the checks on an item of a later slice, the index an item names
//! that does not exist, and the sizes an id may have.

use std::time::Duration;

use oss_writer::bulk::{
    Bulk, BulkBudget, Defaults, IdGenerator, IndexInfo, ItemResult, Owner, Planned, StreamConfig,
    Target, stream, write_response,
};
use oss_writer::index::{Outcome, Refused, Written};
use serde_json::Value;

/// Accepts every write to indexes named `i...`, refuses to know any other
/// index, and remembers the shards it was asked to refresh.
#[derive(Default)]
struct Known {
    refreshed: Vec<(String, u32)>,
    /// Whether deletes find nothing.
    delete_misses: bool,
}

impl Target for Known {
    fn index(&mut self, name: &str) -> Option<IndexInfo> {
        name.starts_with('i').then(|| IndexInfo {
            name: name.into(),
            uuid: "u".into(),
            shards: 2,
            partition_size: 1,
            routing_shards: 2,
            routing_required: false,
            write_block: Default::default(),
            default_pipeline: None,
            final_pipeline: None,
        })
    }

    fn owner(&mut self, _: &IndexInfo, _: u32) -> Owner {
        Owner::Local
    }

    fn apply(&mut self, _: &IndexInfo, _: u32, item: &Planned<'_>) -> Result<Written, Refused> {
        let outcome = if self.delete_misses && item.op.source.is_empty() {
            Outcome::NotFound
        } else {
            Outcome::Created
        };
        Ok(Written {
            outcome,
            version: 1,
            seq_no: 0,
            primary_term: 1,
        })
    }

    fn refresh(&mut self, info: &IndexInfo, shard: u32) -> Result<(), String> {
        self.refreshed.push((info.name.clone(), shard));
        Ok(())
    }

    fn forward(&mut self, _: &str, _: &IndexInfo, _: u32, _: &[Planned<'_>]) -> Vec<ItemResult> {
        unreachable!("every shard is local")
    }
}

/// The response to `body`, and the shards the target refreshed.
fn run(body: &str, slice_bytes: usize, refresh: bool) -> (Value, Vec<(String, u32)>) {
    let budget = BulkBudget::new(1 << 20, 2);
    let config = StreamConfig {
        slice_bytes,
        max_wait: Duration::from_secs(5),
    };
    let mut target = Known::default();
    let mut ids = IdGenerator::new(0, [0; 6]);
    let mut buffer = [0u8; 7];
    let items = {
        let mut bulk = Bulk::new(&mut target, &mut ids, 1, refresh);
        stream(
            &mut body.as_bytes(),
            &mut buffer,
            &Defaults::default(),
            &budget,
            config,
            &mut bulk,
        )
        .unwrap();
        bulk.finish().unwrap()
    };
    let mut out = Vec::new();
    write_response(&mut out, 0, &items).unwrap();
    (serde_json::from_slice(&out).unwrap(), target.refreshed)
}

fn doc(index: &str, id: &str) -> String {
    format!("{{\"index\":{{\"_index\":\"{index}\",\"_id\":\"{id}\"}}}}\n{{\"n\":1}}\n")
}

#[test]
fn a_refresh_reaches_each_written_shard_once_and_marks_the_items() {
    let body: String = (0..6).map(|n| doc("ia", &n.to_string())).collect();
    let (response, refreshed) = run(&body, 1 << 20, true);
    let mut shards: Vec<u32> = refreshed.iter().map(|(_, s)| *s).collect();
    shards.sort_unstable();
    let before = shards.len();
    shards.dedup();
    assert_eq!(
        before,
        shards.len(),
        "a shard is refreshed once: {refreshed:?}"
    );
    assert!(!refreshed.is_empty());
    for item in response["items"].as_array().unwrap() {
        assert_eq!(item["index"]["forced_refresh"], true);
    }
    // Each slice applies on its own: a shard written by two is still
    // refreshed once.
    let (_, refreshed) = run(&body, 1, true);
    let mut seen = refreshed.clone();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), refreshed.len(), "{refreshed:?}");
    // Without the flag nothing is refreshed or marked.
    let (response, refreshed) = run(&body, 1 << 20, false);
    assert!(refreshed.is_empty());
    assert!(
        response["items"][0]["index"]
            .get("forced_refresh")
            .is_none()
    );
}

#[test]
fn a_shard_whose_writes_all_failed_is_not_refreshed() {
    // Only an item that wrote asks for a refresh: this index does not exist.
    let (_, refreshed) = run(&doc("zz", "1"), 1 << 20, true);
    assert!(refreshed.is_empty());
}

#[test]
fn an_item_of_a_later_slice_that_fails_the_checks_fails_alone() {
    // A one-byte slice sends each operation on its own: the second is an
    // `index` with an external version and no id.
    let body = format!(
        "{}{{\"delete\":{{\"_index\":\"ia\"}}}}\n{}",
        doc("ia", "1"),
        doc("ia", "3")
    );
    let (response, _) = run(&body, 1, false);
    let items = response["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["index"]["status"], 201);
    assert_eq!(items[1]["delete"]["status"], 400);
    assert_eq!(
        items[1]["delete"]["error"]["type"],
        "action_request_validation_exception"
    );
    assert_eq!(items[2]["index"]["status"], 201);
}

#[test]
fn a_missing_index_answers_as_stock_answers_it() {
    let missing = |extra: &str| {
        format!("{{\"index\":{{\"_index\":\"zz\",\"_id\":\"1\"{extra}}}}}\n{{\"n\":1}}\n")
    };
    let (plain, _) = run(&missing(""), 1 << 20, false);
    let e = &plain["items"][0]["index"]["error"];
    assert_eq!(plain["items"][0]["index"]["status"], 404);
    assert_eq!(e["type"], "index_not_found_exception");
    assert_eq!(e["reason"], "no such index [zz]");
    assert_eq!(e["resource.type"], "index_expression");
    assert_eq!(e["resource.id"], "zz");
    assert_eq!(e["index_uuid"], "_na_");
    let (aliased, _) = run(&missing(",\"require_alias\":true"), 1 << 20, false);
    let e = &aliased["items"][0]["index"]["error"];
    assert_eq!(
        e["reason"],
        "no such index [zz] and [require_alias] request flag is [true] and [zz] is not an alias"
    );
    assert!(e.get("resource.type").is_none());
    assert!(e.get("resource.id").is_none());
    assert_eq!(e["index"], "zz");
}

#[test]
fn an_id_may_be_512_bytes_and_not_more() {
    let ok = "a".repeat(512);
    let (response, _) = run(&doc("ia", &ok), 1 << 20, false);
    assert_eq!(response["items"][0]["index"]["status"], 201);
    let long = "a".repeat(513);
    let result = std::panic::catch_unwind(|| run(&doc("ia", &long), 1 << 20, false));
    // The whole request is refused, so `run` (which unwraps) panics.
    assert!(result.is_err());
}
