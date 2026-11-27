//! docs/specs/node-doc-write-route: one item as the single-document
//! answer, and its HTTP status.

use oss_rest::Exception;
use oss_writer::bulk::{Item, ItemResult, doc_status, write_doc};
use oss_writer::index::{Outcome, Written};
use serde_json::{Value, json};

fn written(outcome: Outcome, forced_refresh: bool) -> Item {
    Item {
        action: "index",
        index: "docs".into(),
        id: "a\"1".into(),
        result: ItemResult::Written {
            written: Written {
                outcome,
                version: 4,
                seq_no: 9,
                primary_term: 2,
            },
            forced_refresh,
        },
    }
}

fn failed(status: u16) -> Item {
    Item {
        action: "index",
        index: "docs".into(),
        id: "1".into(),
        result: ItemResult::Failed {
            status,
            error: Exception::new("version_conflict_engine_exception", "stale"),
        },
    }
}

fn rendered(item: &Item) -> Value {
    let mut out = Vec::new();
    write_doc(&mut out, item).expect("writes");
    serde_json::from_slice(&out).expect("json")
}

#[test]
fn each_outcome_has_its_status() {
    assert_eq!(doc_status(&written(Outcome::Created, false)), 201);
    assert_eq!(doc_status(&written(Outcome::Updated, false)), 200);
    assert_eq!(doc_status(&written(Outcome::Deleted, false)), 200);
    assert_eq!(doc_status(&written(Outcome::NotFound, false)), 404);
    assert_eq!(doc_status(&failed(409)), 409);
    assert_eq!(doc_status(&failed(400)), 400);
}

#[test]
fn a_written_item_is_the_single_document_answer() {
    assert_eq!(
        rendered(&written(Outcome::Updated, false)),
        json!({
            "_index": "docs",
            "_id": "a\"1",
            "_version": 4,
            "result": "updated",
            "_shards": {"total": 1, "successful": 1, "failed": 0},
            "_seq_no": 9,
            "_primary_term": 2
        })
    );
    let forced = rendered(&written(Outcome::Created, true));
    assert_eq!(forced["forced_refresh"], true);
    assert_eq!(forced["result"], "created");
}

#[test]
fn a_failed_item_is_its_error_and_status() {
    let v = rendered(&failed(409));
    assert_eq!(v["status"], 409);
    assert_eq!(v["error"]["type"], "version_conflict_engine_exception");
    assert_eq!(v["error"]["reason"], "stale");
}
