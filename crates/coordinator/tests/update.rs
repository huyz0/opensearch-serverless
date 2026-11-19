//! docs/specs/coordinator-update: what an update does to the document it read.

// ARITH: test counts are small constants.
#![allow(clippy::arithmetic_side_effects)]

use oss_coordinator::docs::update::{
    Existing, Options, Plan, Refused, Request, ScriptOp, UNASSIGNED_SEQ_NO, WriteCondition,
    attempts, plan, script_operation, validate, write_condition,
};

fn ok() -> Options {
    Options {
        retry_on_conflict: 0,
        scripted_upsert: false,
        has_script: false,
        require_alias: false,
        active_shards_above_one: false,
        extra_field_values: false,
    }
}

fn found(seq: i64, term: i64) -> Existing {
    Existing {
        found: true,
        seq_no: seq,
        primary_term: term,
    }
}

fn req(if_seq_no: i64, if_primary_term: i64) -> Request {
    Request {
        doc_as_upsert: false,
        has_upsert: false,
        has_script: false,
        detect_noop: false,
        if_seq_no,
        if_primary_term,
    }
}

const MISSING: Existing = Existing {
    found: false,
    seq_no: UNASSIGNED_SEQ_NO,
    primary_term: 0,
};

#[test]
fn options_are_refused_in_the_shells_order() {
    assert_eq!(validate(&ok()), Ok(()));
    let all = Options {
        retry_on_conflict: -1,
        scripted_upsert: true,
        has_script: false,
        require_alias: true,
        active_shards_above_one: true,
        extra_field_values: true,
    };
    assert_eq!(
        validate(&all),
        Err(Refused::BadRequest(
            "retry_on_conflict must be non negative"
        ))
    );
    let o = Options {
        retry_on_conflict: 0,
        ..all
    };
    assert_eq!(
        validate(&o),
        Err(Refused::BadRequest(
            "scripted_upsert was given without a script"
        ))
    );
    let o = Options {
        scripted_upsert: false,
        ..o
    };
    let not_implemented = Err(Refused::NotImplemented(
        "require_alias and active-shard counts above one are not supported on updates",
    ));
    assert_eq!(validate(&o), not_implemented);
    let o = Options {
        require_alias: false,
        active_shards_above_one: true,
        ..o
    };
    assert_eq!(validate(&o), not_implemented);
    let o = Options {
        active_shards_above_one: false,
        ..o
    };
    assert_eq!(
        validate(&o),
        Err(Refused::NotImplemented(
            "extra field values are not supported on updates"
        ))
    );
}

#[test]
fn attempts_are_one_plus_the_retries() {
    assert_eq!(attempts(0), 1);
    assert_eq!(attempts(3), 4);
    assert_eq!(attempts(i32::MAX), u64::from(i32::MAX.unsigned_abs()) + 1);
}

#[test]
fn a_changed_document_conflicts_with_a_set_condition() {
    let detect = Request {
        detect_noop: true,
        ..req(5, 2)
    };
    let p = plan("logs", "7", &found(5, 2), &detect);
    assert_eq!(p, Ok(Plan::Merge { detect_noop: true }));
    let p = plan("logs", "7", &found(6, 2), &req(5, 2));
    let want = Err(Refused::Conflict(
        "[7]: version conflict, required seqNo [5], primary term [2]. \
         current document has seqNo [6] and primary term [2]"
            .into(),
    ));
    assert_eq!(p, want);
    let p = plan("logs", "7", &found(5, 3), &req(5, 2));
    assert!(matches!(p, Err(Refused::Conflict(_))));
    // Unset never conflicts, and a missing document never does.
    let p = plan("logs", "7", &found(9, 9), &req(UNASSIGNED_SEQ_NO, 0));
    assert_eq!(p, Ok(Plan::Merge { detect_noop: false }));
    let doc_as_upsert = Request {
        doc_as_upsert: true,
        ..req(5, 2)
    };
    let p = plan("logs", "7", &MISSING, &doc_as_upsert);
    assert_eq!(p, Ok(Plan::Create { from_doc: true }));
}

#[test]
fn a_missing_document_is_created_from_the_doc_or_upsert_or_refused() {
    let p = |doc_as_upsert, has_upsert| {
        let r = Request {
            doc_as_upsert,
            has_upsert,
            ..req(UNASSIGNED_SEQ_NO, 0)
        };
        plan("logs", "7", &MISSING, &r)
    };
    assert_eq!(p(true, true), Ok(Plan::Create { from_doc: true }));
    assert_eq!(p(true, false), Ok(Plan::Create { from_doc: true }));
    assert_eq!(p(false, true), Ok(Plan::Create { from_doc: false }));
    assert_eq!(
        p(false, false),
        Err(Refused::Missing("document missing: [logs]/[7]".into()))
    );
}

#[test]
fn a_found_document_is_scripted_or_merged() {
    let with = |has_script, detect_noop| {
        let r = Request {
            has_script,
            detect_noop,
            ..req(UNASSIGNED_SEQ_NO, 0)
        };
        plan("logs", "7", &found(1, 1), &r)
    };
    assert_eq!(with(true, true), Ok(Plan::Script));
    assert_eq!(with(false, true), Ok(Plan::Merge { detect_noop: true }));
    assert_eq!(with(false, false), Ok(Plan::Merge { detect_noop: false }));
}

#[test]
fn a_scripts_operation_is_kept_only_where_it_makes_sense() {
    use ScriptOp::{Create, Delete, Index, Noop};
    assert_eq!(script_operation(true, "index"), Index);
    assert_eq!(script_operation(true, "delete"), Delete);
    assert_eq!(script_operation(true, "create"), Noop);
    assert_eq!(script_operation(true, "none"), Noop);
    assert_eq!(script_operation(true, "noop"), Noop);
    assert_eq!(script_operation(true, "frobnicate"), Noop);
    assert_eq!(script_operation(false, "create"), Create);
    assert_eq!(script_operation(false, "index"), Noop);
    assert_eq!(script_operation(false, "delete"), Noop);
}

#[test]
fn the_write_carries_the_callers_condition_or_the_one_just_read() {
    let none = UNASSIGNED_SEQ_NO;
    assert_eq!(
        write_condition(&found(4, 1), 9, 2),
        WriteCondition {
            if_seq_no: 9,
            if_primary_term: 2,
            require_absent: false
        }
    );
    assert_eq!(
        write_condition(&found(4, 1), none, 0),
        WriteCondition {
            if_seq_no: 4,
            if_primary_term: 1,
            require_absent: false
        }
    );
    assert_eq!(
        write_condition(&MISSING, none, 0),
        WriteCondition {
            if_seq_no: none,
            if_primary_term: 0,
            require_absent: true
        }
    );
    // A condition the caller set stands even for a missing document.
    assert_eq!(
        write_condition(&MISSING, 3, 1),
        WriteCondition {
            if_seq_no: 3,
            if_primary_term: 1,
            require_absent: false
        }
    );
}

#[test]
fn the_unset_condition_is_the_shells_number() {
    assert_eq!(UNASSIGNED_SEQ_NO, -2);
    // A literal -2 means no condition; any other number is one.
    let p = plan("i", "1", &found(9, 9), &req(-2, 0));
    assert_eq!(p, Ok(Plan::Merge { detect_noop: false }));
    assert!(plan("i", "1", &found(9, 9), &req(-1, 0)).is_err());
}
