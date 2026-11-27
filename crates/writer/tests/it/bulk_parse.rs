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
            write_block: Default::default(),
            default_pipeline: None,
            final_pipeline: None,
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
                ItemResult::Noop { .. } => 200,
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

/// Gives `first` once, then the body.
struct FailsOnce<'a> {
    first: Option<std::io::ErrorKind>,
    body: &'a [u8],
}

impl std::io::Read for FailsOnce<'_> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if let Some(kind) = self.first.take() {
            return Err(kind.into());
        }
        self.body.read(out)
    }
}

fn stream_from(reader: &mut dyn std::io::Read) -> Result<usize, u16> {
    let budget = BulkBudget::new(1 << 20, 2);
    let config = StreamConfig {
        slice_bytes: 1 << 20,
        max_wait: Duration::from_secs(5),
    };
    let mut target = Accept;
    let mut ids = IdGenerator::new(0, [0; 6]);
    let mut buffer = [0u8; 64];
    let mut bulk = Bulk::new(&mut target, &mut ids, 1, false);
    stream(
        reader,
        &mut buffer,
        &with_index(),
        &budget,
        config,
        &mut bulk,
    )
    .and_then(|()| bulk.finish())
    .map(|items| items.len())
    .map_err(|e| e.status())
}

#[test]
fn an_interrupted_read_is_retried_and_any_other_failure_refuses_the_body() {
    let body = b"{\"index\":{\"_id\":\"1\"}}\n{\"n\":1}\n";
    let mut interrupted = FailsOnce {
        first: Some(std::io::ErrorKind::Interrupted),
        body,
    };
    assert_eq!(stream_from(&mut interrupted), Ok(1));
    let mut broken = FailsOnce {
        first: Some(std::io::ErrorKind::BrokenPipe),
        body,
    };
    assert_eq!(stream_from(&mut broken), Err(400));
}

#[test]
fn an_object_source_in_the_action_is_skipped_whatever_its_depth() {
    // Stock reads the action's `_source` as a nested value and moves on to
    // the parameters after it.
    for line in [
        r#"{"delete":{"_source":{"a":{"b":[1,{"c":2}]},"d":[]},"_id":"1"}}"#,
        r#"{"delete":{"_source":{},"_id":"1"}}"#,
        r#"{"delete":{"_source":{"a":[[1],[2]]},"_id":"1"}}"#,
    ] {
        let body = format!("{line}\n");
        assert_eq!(run(&body, &with_index()), Ok(vec![201]), "{line}");
    }
}

#[test]
fn an_object_given_for_any_other_parameter_is_refused() {
    let body = "{\"index\":{\"routing\":{\"x\":1},\"_id\":\"1\"}}\n{}\n";
    assert!(run(body, &with_index()).is_err());
}

#[test]
fn a_create_refusal_stops_the_item_s_other_checks() {
    // `IndexRequest.validate` returns at a create refusal: the empty
    // pipeline is not reported.
    let body = "{\"create\":{\"_id\":\"1\",\"version\":3,\"version_type\":\"external\",\"pipeline\":\"\"}}\n{}\n";
    assert_eq!(
        run(body, &with_index()),
        Err(
            "Validation Failed: 1: create operations only support internal versioning. use index instead;"
                .to_owned()
        )
    );
}

#[test]
fn an_index_refusal_does_not_stop_the_item_s_other_checks() {
    let body = "{\"index\":{\"_id\":\"1\",\"version\":-5,\"version_type\":\"external\",\"pipeline\":\"\"}}\n{}\n";
    assert_eq!(
        run(body, &with_index()),
        Err(
            "Validation Failed: 1: illegal version value [-5] for version type [EXTERNAL];2: pipeline cannot be an empty string;"
                .to_owned()
        )
    );
}

#[test]
fn a_version_without_an_id_asks_for_one() {
    let need = "an id must be provided if version type or value are set";
    for line in [
        r#"{"index":{"version":5,"version_type":"external"}}"#,
        r#"{"index":{"version":-3,"version_type":"external_gte"}}"#,
    ] {
        let body = format!("{line}\n{{}}\n");
        let Err(reason) = run(&body, &with_index()) else {
            panic!("{line} was accepted");
        };
        assert!(reason.contains(need), "{line}: {reason}");
    }
    // An internal version with no id is refused for its version, and
    // asks for an id too.
    let body = "{\"index\":{\"version\":5}}\n{}\n";
    let Err(reason) = run(body, &with_index()) else {
        panic!("accepted");
    };
    assert!(
        reason.starts_with(&format!("Validation Failed: 1: {need};")),
        "{reason}"
    );
    // No version, or a create, needs none.
    for line in [r#"{"index":{}}"#, r#"{"create":{}}"#] {
        let body = format!("{line}\n{{}}\n");
        assert_eq!(run(&body, &with_index()), Ok(vec![201]), "{line}");
    }
}

#[test]
fn a_create_that_passes_its_version_checks_gets_the_rest() {
    let body = "{\"create\":{\"_id\":\"1\",\"pipeline\":\"\"}}\n{}\n";
    assert_eq!(
        run(body, &with_index()),
        Err("Validation Failed: 1: pipeline cannot be an empty string;".to_owned())
    );
}
