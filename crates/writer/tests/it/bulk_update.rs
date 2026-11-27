//! docs/specs/node-bulk-update: an `update` item is resolved through the
//! node's `Updates` and applied as the write it became.

use oss_rest::Exception;
use oss_writer::bulk::{
    Action, Bulk, IdGenerator, IndexInfo, ItemResult, Kind, Op, Owner, Planned, Resolved, Target,
    UpdateFailure, Updates, write_response,
};
use oss_writer::index::{Expect, Outcome, Refused, UNASSIGNED_SEQ_NO, Written};

/// What was applied: kind, create, `if_seq_no`, `if_primary_term`, source.
type Applied = (Kind, bool, i64, i64, Vec<u8>);

#[derive(Default)]
struct Log(Vec<Applied>);

impl Target for Log {
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

    fn apply(&mut self, _: &IndexInfo, _: u32, p: &Planned<'_>) -> Result<Written, Refused> {
        let a = &p.op.action;
        self.0.push((
            a.kind,
            a.create,
            a.expect.if_seq_no,
            a.expect.if_primary_term,
            p.op.source.clone(),
        ));
        Ok(Written {
            outcome: Outcome::Updated,
            version: 5,
            seq_no: 9,
            primary_term: 2,
        })
    }

    fn refresh(&mut self, _: &IndexInfo, _: u32) -> Result<(), String> {
        Ok(())
    }

    fn forward(&mut self, _: &str, _: &IndexInfo, _: u32, _: &[Planned<'_>]) -> Vec<ItemResult> {
        unreachable!()
    }
}

/// Answers by the id of the item.
struct Fake;

impl Updates for Fake {
    fn resolve(
        &mut self,
        _: &IndexInfo,
        _: u32,
        _: &Action,
        id: &str,
        body: &[u8],
    ) -> Result<Resolved, UpdateFailure> {
        match id {
            "put" => Ok(Resolved::Put {
                source: body.to_vec(),
                create: false,
                if_seq_no: 4,
                if_primary_term: 2,
            }),
            "delete" => Ok(Resolved::Delete {
                if_seq_no: 4,
                if_primary_term: 2,
            }),
            "noop" => Ok(Resolved::Noop {
                version: 3,
                seq_no: 4,
                primary_term: 2,
            }),
            _ => Err(UpdateFailure {
                status: 404,
                error: Exception::new("document_missing_exception", "missing"),
            }),
        }
    }
}

fn update(id: &str, body: &str) -> Op {
    Op {
        action: Action {
            kind: Kind::Update,
            index: Some("docs".into()),
            id: Some(id.into()),
            routing: None,
            create: false,
            expect: Expect {
                if_seq_no: UNASSIGNED_SEQ_NO,
                ..Expect::default()
            },
            pipeline: None,
            require_alias: false,
        },
        source: body.as_bytes().to_vec(),
    }
}

fn run(ops: &[Op], updates: Option<&mut Fake>) -> (Log, Vec<oss_writer::bulk::Item>) {
    let mut target = Log::default();
    let mut ids = IdGenerator::new(0, [0; 6]);
    let items = {
        let mut bulk = Bulk::new(&mut target, &mut ids, 1, false);
        if let Some(u) = updates {
            bulk = bulk.with_updates(u);
        }
        bulk.slice(ops).expect("slice");
        bulk.finish().expect("finish")
    };
    (target, items)
}

#[test]
fn an_update_is_applied_as_the_write_it_resolved_to() {
    let ops = [
        update("put", r#"{"n":1}"#),
        update("delete", "{}"),
        update("noop", "{}"),
        update("nope", "{}"),
    ];
    let (log, items) = run(&ops, Some(&mut Fake));
    assert_eq!(
        log.0,
        vec![
            (Kind::Index, false, 4, 2, br#"{"n":1}"#.to_vec()),
            (Kind::Delete, false, 4, 2, Vec::new()),
        ]
    );
    assert!(items.iter().all(|i| i.action == "update"));
    assert!(matches!(items[0].result, ItemResult::Written { .. }));
    assert!(matches!(items[1].result, ItemResult::Written { .. }));
    assert_eq!(
        items[2].result,
        ItemResult::Noop {
            version: 3,
            seq_no: 4,
            primary_term: 2
        }
    );
    assert!(matches!(
        &items[3].result,
        ItemResult::Failed { status: 404, error } if error.kind() == "document_missing_exception"
    ));
    let mut out = Vec::new();
    write_response(&mut out, 0, &items).unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains(r#""update":{"_index":"docs","_id":"noop","_version":3,"result":"noop","_shards":{"total":0,"successful":0,"failed":0},"_seq_no":4,"_primary_term":2,"status":200}"#), "{text}");
    assert!(text.contains(r#""errors":true"#), "{text}");
}

#[test]
fn a_second_item_for_one_document_in_a_slice_is_a_conflict() {
    let ops = [update("put", "{}"), update("put", "{}")];
    let (log, items) = run(&ops, Some(&mut Fake));
    assert_eq!(log.0.len(), 1, "the first is written");
    assert!(matches!(items[0].result, ItemResult::Written { .. }));
    assert!(matches!(
        &items[1].result,
        ItemResult::Failed { status: 409, error }
            if error.kind() == "version_conflict_engine_exception"
                && error.reason().contains("earlier item of this bulk")
    ));
}

#[test]
fn without_updates_an_update_item_is_not_supported() {
    let (log, items) = run(&[update("put", "{}")], None);
    assert!(log.0.is_empty());
    assert!(matches!(
        &items[0].result,
        ItemResult::Failed { status: 501, .. }
    ));
}
