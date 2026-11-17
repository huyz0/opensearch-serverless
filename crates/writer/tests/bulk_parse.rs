//! Action lines as `BulkRequestParser` reads them, each case answered by
//! stock OpenSearch 3.10 as recorded in the comments: the request's refusal
//! or each item's status.

use std::time::Duration;

use oss_writer::bulk::{
    Bulk, BulkBudget, Defaults, IdGenerator, IndexInfo, ItemResult, Owner, Planned, StreamConfig,
    Target, stream,
};
use oss_writer::index::{Outcome, Refused, Written};
use serde_json::Value;

struct Accept;

impl Target for Accept {
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

    fn apply(&mut self, _: &IndexInfo, _: u32, _: &Planned<'_>) -> Result<Written, Refused> {
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
        unreachable!()
    }
}

/// `Ok(statuses)` of the items, or `Err(reason)` of the request's refusal.
fn run(body: &str, defaults: &Defaults) -> Result<Vec<u16>, String> {
    let budget = BulkBudget::new(1 << 20, 2);
    let config = StreamConfig {
        slice_bytes: 1 << 20,
        max_wait: Duration::from_secs(5),
    };
    let mut target = Accept;
    let mut ids = IdGenerator::new(0, [0; 6]);
    let mut buffer = [0u8; 5];
    let mut bulk = Bulk::new(&mut target, &mut ids, 1, false);
    let result = stream(
        &mut body.as_bytes(),
        &mut buffer,
        defaults,
        &budget,
        config,
        &mut bulk,
    )
    .and_then(|()| bulk.finish());
    match result {
        Ok(items) => Ok(items
            .iter()
            .map(|i| match &i.result {
                ItemResult::Written { .. } => 201,
                ItemResult::Failed { status, .. } => *status,
            })
            .collect()),
        Err(e) => {
            let mut out = Vec::new();
            e.write_body(&mut out).unwrap();
            let v: Value = serde_json::from_slice(&out).unwrap();
            Err(v["error"]["reason"].as_str().unwrap().to_owned())
        }
    }
}

fn with_index() -> Defaults {
    Defaults {
        index: Some("i".into()),
        ..Defaults::default()
    }
}

#[test]
fn scalars_are_coerced_as_jacksons_accessors_coerce_them() {
    for line in [
        // A string version, a fractional one and a numeric id were accepted.
        r#"{"index":{"_id":"1","version":"5","version_type":"external"}}"#,
        r#"{"index":{"_id":"2","version":1.5,"version_type":"external"}}"#,
        r#"{"index":{"_id":7}}"#,
        // An unknown op_type is an index; null routing and `retry_on_conflict` are read.
        r#"{"index":{"_id":"9","op_type":"foo"}}"#,
        r#"{"index":{"_id":"6","routing":null,"retry_on_conflict":3}}"#,
        // A scalar `_source` and text after the action's object are ignored.
        r#"{"index":{"_id":"4","_source":true}}"#,
        r#"{"index":{"_id":"12"}} garbage"#,
    ] {
        let body = format!("{line}\n{{\"n\":1}}\n");
        assert_eq!(run(&body, &with_index()), Ok(vec![201]), "{line}");
    }
}

#[test]
fn refusals_carry_the_javas_messages() {
    for (line, reason) in [
        (
            r#"{"index":{"_id":"3","require_alias":"yes"}}"#,
            "Failed to parse value [yes] as only [true] or [false] are allowed.",
        ),
        (
            r#"{"index":{"_id":"8","if_seq_no":"abc","if_primary_term":1}}"#,
            "For input string: \"abc\"",
        ),
        (
            r#"{"index":{"_id":"11","pipeline":""}}"#,
            "Validation Failed: 1: pipeline cannot be an empty string;",
        ),
        (
            "{}",
            "Malformed action/metadata line [1], expected FIELD_NAME but found [END_OBJECT]",
        ),
        (
            r#"{"index":{"_id":"1","version_type":"sideways"}}"#,
            "No version type match [sideways]",
        ),
        (
            r#"{"index":{"_id":"1","tags":["a"]}}"#,
            "Malformed action/metadata line [1], expected a simple value for field [tags] but found [START_ARRAY]",
        ),
    ] {
        let body = format!("{line}\n{{\"n\":1}}\n");
        assert_eq!(run(&body, &with_index()), Err(reason.into()), "{line}");
    }
}

#[test]
fn an_action_with_no_index_anywhere_is_refused() {
    let body = "{\"index\":{}}\n\n";
    assert_eq!(
        run(body, &Defaults::default()),
        Err("Validation Failed: 1: index is missing;".into())
    );
}

#[test]
fn request_defaults_apply_to_every_line() {
    let defaults = Defaults {
        index: Some("i".into()),
        routing: Some("r".into()),
        pipeline: Some("p".into()),
        require_alias: false,
    };
    // The request's pipeline names one that does not exist.
    assert_eq!(
        run("{\"index\":{\"_id\":\"1\"}}\n{\"n\":1}\n", &defaults),
        Ok(vec![400])
    );
    // An action line's own `pipeline: _none` turns it off.
    assert_eq!(
        run(
            "{\"index\":{\"_id\":\"1\",\"pipeline\":\"_none\"}}\n{\"n\":1}\n",
            &defaults
        ),
        Ok(vec![201])
    );
}

#[test]
fn a_delete_takes_no_source_line() {
    assert_eq!(
        run(
            "{\"delete\":{\"_id\":\"1\"}}\n{\"delete\":{\"_id\":\"2\"}}\n",
            &with_index()
        ),
        Ok(vec![201, 201])
    );
}
