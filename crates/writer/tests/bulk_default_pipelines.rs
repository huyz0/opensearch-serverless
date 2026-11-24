//! docs/specs/index-default-pipelines: an index's default and final pipelines.

use std::time::Duration;

use oss_writer::bulk::{
    Bulk, BulkBudget, Defaults, IdGenerator, IndexInfo, Ingest, IngestError, ItemResult, Owner,
    Planned, StreamConfig, Target, stream, write_response,
};
use oss_writer::index::{Outcome, Refused, Written};
use serde_json::Value;

/// An index with the given default and final pipelines; remembers what it applied.
struct Records {
    default: Option<&'static str>,
    last: Option<&'static str>,
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
            default_pipeline: self.default.map(str::to_owned),
            final_pipeline: self.last.map(str::to_owned),
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

/// `up` upper-cases, `low` lower-cases, `bad` fails, any other id is missing.
struct Fake;

impl Ingest for Fake {
    fn run(&mut self, id: &str, source: &[u8]) -> Result<Vec<u8>, IngestError> {
        match id {
            "up" => Ok(source.to_ascii_uppercase()),
            "low" => Ok(source.to_ascii_lowercase()),
            "bad" => Err(IngestError::Failed("it broke".into())),
            _ => Err(IngestError::Missing),
        }
    }
}

fn run(
    default: Option<&'static str>,
    last: Option<&'static str>,
    body: &str,
) -> (Value, Vec<(String, String)>) {
    let budget = BulkBudget::new(1 << 20, 2);
    let config = StreamConfig {
        slice_bytes: 40,
        max_wait: Duration::from_secs(5),
    };
    let mut target = Records {
        default,
        last,
        applied: Vec::new(),
    };
    let mut ids = IdGenerator::new(0, [0; 6]);
    let mut fake = Fake;
    let mut buffer = [0u8; 16];
    let items = {
        let mut bulk = Bulk::new(&mut target, &mut ids, 1, false).with_ingest(&mut fake);
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
    (serde_json::from_slice(&out).unwrap(), target.applied)
}

fn item(id: &str, pipeline: Option<&str>) -> String {
    let named = pipeline.map_or(String::new(), |p| format!(",\"pipeline\":\"{p}\""));
    format!("{{\"index\":{{\"_id\":\"{id}\"{named}}}}}\n{{\"a\":\"X\"}}\n")
}

fn applied(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(id, source)| ((*id).to_owned(), (*source).to_owned()))
        .collect()
}

#[test]
fn the_default_runs_unless_a_pipeline_is_named_or_none() {
    let body = format!(
        "{}{}{}",
        item("1", None),
        item("2", Some("low")),
        item("3", Some("_none"))
    );
    let (response, done) = run(Some("up"), None, &body);
    assert_eq!(response["errors"], false);
    assert_eq!(
        done,
        applied(&[
            ("1", "{\"A\":\"X\"}"),
            ("2", "{\"a\":\"x\"}"),
            ("3", "{\"a\":\"X\"}")
        ])
    );
}

#[test]
fn the_final_runs_after_whatever_the_write_named() {
    let body = format!(
        "{}{}{}",
        item("1", None),
        item("2", Some("low")),
        item("3", Some("_none"))
    );
    // The first upper-cases and the final lower-cases, so a final that ran
    // first would leave upper case.
    let (_, done) = run(Some("up"), Some("low"), &body);
    assert_eq!(
        done,
        applied(&[
            ("1", "{\"a\":\"x\"}"),
            ("2", "{\"a\":\"x\"}"),
            ("3", "{\"a\":\"x\"}")
        ])
    );
    // `_none` as a setting runs nothing.
    let (_, none) = run(Some("_none"), Some("_none"), &item("1", None));
    assert_eq!(none, applied(&[("1", "{\"a\":\"X\"}")]));
    // The final alone, over the order's other direction.
    let (_, up) = run(None, Some("up"), &item("1", Some("low")));
    assert_eq!(up, applied(&[("1", "{\"A\":\"X\"}")]));
}

#[test]
fn a_missing_default_or_final_fails_its_item_and_a_delete_runs_neither() {
    let (response, done) = run(Some("nope"), None, &item("1", None));
    assert_eq!(response["errors"], true);
    assert_eq!(response["items"][0]["index"]["status"], 400);
    assert!(
        response["items"][0]["index"]["error"]["reason"]
            .as_str()
            .unwrap()
            .contains("pipeline with id [nope] does not exist")
    );
    assert!(done.is_empty());
    let (response, done) = run(None, Some("bad"), &item("1", Some("up")));
    assert_eq!(response["items"][0]["index"]["status"], 400);
    assert_eq!(response["items"][0]["index"]["error"]["reason"], "it broke");
    assert!(done.is_empty());
    let delete = "{\"delete\":{\"_id\":\"1\"}}\n";
    let (response, _) = run(Some("nope"), Some("nope"), delete);
    assert_eq!(response["errors"], false, "{response}");
}
