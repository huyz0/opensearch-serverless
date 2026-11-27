//! What one attempt of an update does: read the document, plan, and decide
//! the write. The update route and a bulk `update` item share it.

use oss_coordinator::docs::update::{
    self, Existing, Options, Plan, Refused, Request as Ask, ScriptOp, UNASSIGNED_SEQ_NO,
};
use oss_coordinator::docs::update_script::run_update;
use oss_doc::mapping::json::{Json, parse};
use oss_rest::Exception;
use oss_writer::bulk::{IndexInfo, Resolved, UpdateFailure};
use oss_writer::index::Fetched;

use crate::update_body::{UpdateBody, merge};
use crate::writer::WriterRole;

/// A failed update: its status and the error answered.
pub(crate) type Failure = UpdateFailure;

/// An update refused with `status` and the OpenSearch `kind`.
pub(crate) fn fail(status: u16, kind: &'static str, reason: impl Into<String>) -> Failure {
    UpdateFailure {
        status,
        error: Exception::new(kind, reason),
    }
}

/// 400 `illegal_argument_exception`.
pub(crate) fn bad(reason: impl Into<String>) -> Failure {
    fail(400, "illegal_argument_exception", reason)
}

fn refused(r: Refused) -> Failure {
    match r {
        Refused::BadRequest(m) => bad(m),
        Refused::NotImplemented(m) => fail(501, "unsupported_operation_exception", m),
        Refused::Conflict(m) => fail(409, "version_conflict_engine_exception", m),
        Refused::Missing(m) => fail(404, "document_missing_exception", m),
    }
}

fn text(json: &Json) -> Vec<u8> {
    // ALLOC: one document; bounded by the request body and the stored source.
    json.to_json().into_bytes()
}

/// The refusals that depend on the request alone.
pub(crate) fn check(
    body: &UpdateBody,
    retry_on_conflict: i32,
    require_alias: bool,
) -> Result<(), Failure> {
    let has_script = body.script.is_some();
    if has_script && !oss_coordinator::docs::update_script::enabled() {
        return Err(fail(
            501,
            "unsupported_operation_exception",
            "script updates need serverless.scripting.painless.native_compile",
        ));
    }
    update::validate(&Options {
        retry_on_conflict,
        scripted_upsert: body.scripted_upsert,
        has_script,
        require_alias,
        active_shards_above_one: false,
        extra_field_values: false,
    })
    .map_err(refused)
}

/// What the plan makes of the document as read.
fn decide(
    body: &UpdateBody,
    plan: Plan,
    index: &str,
    id: &str,
    read: Option<&Fetched>,
) -> Result<Option<(Vec<u8>, bool, bool)>, Failure> {
    // The result: the source to write, whether to create, whether to delete.
    let script_run = |found: bool, version: i64, doc: &[u8]| {
        let script = body
            .script
            .as_ref()
            .ok_or_else(|| bad("scripted_upsert was given without a script"))?;
        run_update(
            &script.source,
            &script.params,
            found,
            index,
            id,
            version,
            doc,
        )
        // ALLOC: error path; one message.
        .map_err(|reason| fail(400, "script_exception", reason))
    };
    Ok(match (plan, read) {
        (Plan::Create { from_doc }, _) => {
            let base = if from_doc { &body.doc } else { &body.upsert };
            let base = base.clone().unwrap_or(Json::Obj(Vec::new()));
            if body.scripted_upsert && body.script.is_some() {
                let out = script_run(false, 0, &text(&base))?;
                (out.op == ScriptOp::Create).then_some((out.source, true, false))
            } else {
                Some((text(&base), true, false))
            }
        }
        (Plan::Script, Some(found)) => {
            let out = script_run(true, found.version, &found.source)?;
            match out.op {
                ScriptOp::Index => Some((out.source, false, false)),
                ScriptOp::Delete => Some((Vec::new(), false, true)),
                ScriptOp::Create | ScriptOp::Noop => None,
            }
        }
        (Plan::Merge { detect_noop }, Some(found)) => {
            let Ok(current @ Json::Obj(_)) = parse(&found.source) else {
                return Err(fail(
                    500,
                    "illegal_state_exception",
                    "the stored source is not an object",
                ));
            };
            let mut merged = current.clone();
            if let Some(doc) = &body.doc {
                merge(&mut merged, doc);
            }
            (!(detect_noop && merged == current)).then_some((text(&merged), false, false))
        }
        (Plan::Script | Plan::Merge { .. }, None) => {
            return Err(fail(
                500,
                "illegal_state_exception",
                "an update planned over a missing document",
            ));
        }
    })
}

/// One attempt: reads the document of `index` and decides what the update
/// writes, carrying the sequence number just read.
pub(crate) fn resolve_once(
    role: &WriterRole,
    index: &IndexInfo,
    shard: u32,
    id: &str,
    body: &UpdateBody,
    if_seq_no: i64,
    if_primary_term: i64,
) -> Result<Resolved, Failure> {
    let found = role
        .get_owned(index, shard, id, true)
        .map_err(|reason| fail(503, "no_shard_available_action_exception", reason))?;
    let existing = Existing {
        found: found.is_some(),
        seq_no: found.as_ref().map_or(0, |f| f.seq_no),
        primary_term: found.as_ref().map_or(0, |f| f.primary_term),
    };
    let ask = Ask {
        doc_as_upsert: body.doc_as_upsert,
        has_upsert: body.upsert.is_some(),
        has_script: body.script.is_some(),
        detect_noop: body.detect_noop,
        if_seq_no,
        if_primary_term,
    };
    let plan = update::plan(&index.name, id, &existing, &ask).map_err(refused)?;
    let cond = update::write_condition(&existing, if_seq_no, if_primary_term);
    Ok(match decide(body, plan, &index.name, id, found.as_ref())? {
        None => Resolved::Noop {
            version: found.as_ref().map_or(0, |f| f.version),
            seq_no: existing.seq_no,
            primary_term: existing.primary_term,
        },
        Some((_, _, true)) => Resolved::Delete {
            if_seq_no: cond.if_seq_no,
            if_primary_term: cond.if_primary_term,
        },
        Some((source, create, false)) => Resolved::Put {
            source,
            create: create || cond.require_absent,
            if_seq_no: cond.if_seq_no,
            if_primary_term: cond.if_primary_term,
        },
    })
}

/// `if_seq_no` when a request sets none.
pub(crate) const UNASSIGNED: i64 = UNASSIGNED_SEQ_NO;
