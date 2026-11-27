//! docs/specs/index-blocks: an index's write block refuses a bulk's items.

use std::time::Duration;

use oss_writer::bulk::{
    Bulk, BulkBudget, Defaults, IdGenerator, IndexInfo, ItemResult, Owner, Planned, StreamConfig,
    Target, WriteBlock, stream, write_response,
};
use oss_writer::index::{Outcome, Refused, Written};
use serde_json::Value;

struct Blocked {
    block: WriteBlock,
    applied: usize,
}

impl Target for Blocked {
    fn index(&mut self, name: &str) -> Option<IndexInfo> {
        Some(IndexInfo {
            name: name.into(),
            uuid: "u".into(),
            shards: 1,
            partition_size: 1,
            routing_shards: 1,
            routing_required: false,
            write_block: self.block,
            default_pipeline: None,
            final_pipeline: None,
        })
    }

    fn owner(&mut self, _: &IndexInfo, _: u32) -> Owner {
        Owner::Local
    }

    fn apply(&mut self, _: &IndexInfo, _: u32, _: &Planned<'_>) -> Result<Written, Refused> {
        self.applied = self.applied.saturating_add(1);
        Ok(Written {
            outcome: Outcome::Created,
            version: 1,
            seq_no: 0,
            primary_term: 1,
        })
    }

    fn refresh(&mut self, _: &IndexInfo, _: u32) -> Result<(), String> {
        Ok(())
    }

    fn forward(&mut self, _: &str, _: &IndexInfo, _: u32, _: &[Planned<'_>]) -> Vec<ItemResult> {
        unreachable!("every shard is local")
    }
}

const BODY: &str = "{\"index\":{\"_id\":\"1\"}}\n{\"a\":1}\n\
{\"create\":{\"_id\":\"2\"}}\n{\"a\":1}\n\
{\"delete\":{\"_id\":\"4\"}}\n";

fn run(block: WriteBlock) -> (Value, usize) {
    let budget = BulkBudget::new(1 << 20, 2);
    let config = StreamConfig {
        slice_bytes: 64,
        max_wait: Duration::from_secs(5),
    };
    let mut target = Blocked { block, applied: 0 };
    let mut ids = IdGenerator::new(0, [0; 6]);
    let mut buffer = [0u8; 16];
    let items = {
        let mut bulk = Bulk::new(&mut target, &mut ids, 1, false);
        stream(
            &mut BODY.as_bytes(),
            &mut buffer,
            &Defaults {
                index: Some("i".into()),
                ..Defaults::default()
            },
            &budget,
            config,
            &mut bulk,
        )
        .unwrap();
        bulk.finish().unwrap()
    };
    let mut out = Vec::new();
    write_response(&mut out, 0, &items).unwrap();
    (serde_json::from_slice(&out).unwrap(), target.applied)
}

/// The status and error type of each item.
fn outcomes(response: &Value) -> Vec<(u64, String)> {
    response["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            let body = item.as_object().unwrap().values().next().unwrap();
            let kind = body["error"]["type"].as_str().unwrap_or("").to_owned();
            (body["status"].as_u64().unwrap(), kind)
        })
        .collect()
}

// AC-1
#[test]
fn a_full_block_refuses_every_item_and_writes_nothing() {
    let (response, applied) = run(WriteBlock::Full);
    assert_eq!(applied, 0);
    let blocked = (403, "cluster_block_exception".to_owned());
    assert_eq!(outcomes(&response), vec![blocked; 3]);
    assert_eq!(
        response["items"][0]["index"]["error"]["reason"],
        "index [i] blocked by: [FORBIDDEN/8/index write (api)];"
    );
}

// AC-2
#[test]
fn an_allow_delete_block_refuses_all_but_a_delete() {
    let (response, applied) = run(WriteBlock::AllowDelete);
    assert_eq!(applied, 1);
    let blocked = (403, "cluster_block_exception".to_owned());
    let got = outcomes(&response);
    assert_eq!(got[..2], [blocked.clone(), blocked]);
    assert_eq!(got[2].1, "");
    assert!(
        response["items"][0]["index"]["error"]["reason"]
            .as_str()
            .unwrap()
            .contains("FORBIDDEN/12/")
    );
}

// AC-3 at the writer: no block, nothing refused.
#[test]
fn no_block_refuses_nothing() {
    let (response, applied) = run(WriteBlock::None);
    assert_eq!(applied, 3);
    assert_eq!(response["errors"], false);
}
