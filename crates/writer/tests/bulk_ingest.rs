//! Items that name a pipeline: the document an ingest made is the one the
//! target applies, a pipeline that is missing or fails fails that item
//! alone, and `_none` and a delete skip it.

use std::time::Duration;

use oss_writer::bulk::{
    Bulk, BulkBudget, Defaults, IdGenerator, IndexInfo, Ingest, IngestError, ItemResult, Owner,
    Planned, StreamConfig, Target, stream, write_response,
};
use oss_writer::index::{Outcome, Refused, Written};
use serde_json::Value;

/// Accepts every write and remembers the id and source of each.
#[derive(Default)]
struct Records {
    applied: Vec<(String, String)>,
}

impl Target for Records {
    fn index(&mut self, name: &str) -> Option<IndexInfo> {
        Some(IndexInfo {
            name: name.into(),
            uuid: "u".into(),
            shards: 1,
            partition_size: 1,
            routing_shards: 1,
            routing_required: false,
        })
    }

    fn owner(&mut self, _: &IndexInfo, _: u32) -> Owner {
        Owner::Local
    }

    fn apply(&mut self, _: &IndexInfo, _: u32, item: &Planned<'_>) -> Result<Written, Refused> {
        self.applied.push((
            item.id.to_owned(),
            String::from_utf8(item.op.source.clone()).unwrap(),
        ));
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

/// `up` upper-cases the source, `bad` fails, any other id is missing; it
/// counts its calls.
#[derive(Default)]
struct Fake {
    calls: usize,
}

impl Ingest for Fake {
    fn run(&mut self, id: &str, source: &[u8]) -> Result<Vec<u8>, IngestError> {
        self.calls = self.calls.saturating_add(1);
        match id {
            "up" => Ok(source.to_ascii_uppercase()),
            "bad" => Err(IngestError::Failed("it broke".into())),
            _ => Err(IngestError::Missing),
        }
    }
}

/// The response, the applied writes and the ingest's call count.
fn run(body: &str, with: bool) -> (Value, Vec<(String, String)>, usize) {
    let budget = BulkBudget::new(1 << 20, 2);
    let config = StreamConfig {
        slice_bytes: 40,
        max_wait: Duration::from_secs(5),
    };
    let mut target = Records::default();
    let mut ids = IdGenerator::new(0, [0; 6]);
    let mut fake = Fake::default();
    let mut buffer = [0u8; 16];
    let items = {
        let bulk = Bulk::new(&mut target, &mut ids, 1, false);
        let mut bulk = if with {
            bulk.with_ingest(&mut fake)
        } else {
            bulk
        };
        stream(
            &mut body.as_bytes(),
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
    (serde_json::from_slice(&out).unwrap(), target.applied, fake.calls)
}

fn item(id: &str, pipeline: &str) -> String {
    format!("{{\"index\":{{\"_id\":\"{id}\",\"pipeline\":\"{pipeline}\"}}}}\n{{\"a\":\"x\"}}\n")
}

#[test]
fn the_document_a_pipeline_made_is_the_one_applied() {
    let body = format!("{}{}", item("1", "up"), item("2", "_none"));
    let (response, applied, calls) = run(&body, true);
    assert_eq!(response["errors"], false);
    assert_eq!(
        applied,
        vec![
            ("1".to_owned(), "{\"A\":\"X\"}".to_owned()),
            ("2".to_owned(), "{\"a\":\"x\"}".to_owned()),
        ]
    );
    assert_eq!(calls, 1, "`_none` runs nothing");
}

#[test]
fn a_missing_or_failing_pipeline_fails_that_item_alone() {
    let body = format!("{}{}{}", item("1", "nope"), item("2", "bad"), item("3", "up"));
    let (response, applied, _) = run(&body, true);
    let items = response["items"].as_array().unwrap();
    assert_eq!(response["errors"], true);
    assert_eq!(items[0]["index"]["status"], 400);
    assert_eq!(
        items[0]["index"]["error"]["reason"],
        "pipeline with id [nope] does not exist"
    );
    assert_eq!(items[1]["index"]["status"], 400);
    assert_eq!(items[1]["index"]["error"]["reason"], "it broke");
    assert_eq!(items[2]["index"]["status"], 201);
    assert_eq!(applied.len(), 1);
}

#[test]
fn a_delete_skips_its_pipeline_and_without_ingest_a_pipeline_is_missing() {
    let body = "{\"delete\":{\"_id\":\"1\",\"pipeline\":\"up\"}}\n";
    let (response, _, calls) = run(body, true);
    assert_eq!(response["items"][0]["delete"]["status"], 201);
    assert_eq!(calls, 0);
    let (response, applied, _) = run(&item("1", "up"), false);
    assert_eq!(response["items"][0]["index"]["status"], 400);
    assert!(applied.is_empty());
}
