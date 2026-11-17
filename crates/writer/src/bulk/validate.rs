//! `BulkRequest.validate`: every item's own checks, gathered into one
//! `action_request_validation_exception` for the request.

use crate::index::{MATCH_ANY, MATCH_DELETED, VersionType, version_errors};

use super::action::Kind;
use super::reader::Op;

/// `DocWriteRequest.validateDocIdLength`'s bound, in UTF-8 bytes.
pub const MAX_ID_BYTES: usize = 512;

/// Adds the checks `op`'s request makes (`IndexRequest.validate`,
/// `DeleteRequest.validate`, `UpdateRequest.validate`) to `errors`, in the
/// Java's order.
pub fn op_errors(op: &Op, errors: &mut Vec<String>) {
    let a = &op.action;
    if a.index.is_none() {
        errors.push("index is missing".into());
    }
    match a.kind {
        Kind::Delete | Kind::Update => {
            if a.id.as_deref().is_none_or(str::is_empty) {
                errors.push("id is missing".into());
            }
            version_errors(&a.expect, false, errors);
        }
        Kind::Index | Kind::Create => {
            let create = a.create;
            let before = errors.len();
            version_errors(&a.expect, create, errors);
            let create_refused = create && errors.len() > before;
            if create_refused {
                // `IndexRequest.validate` returns at a create refusal.
                return;
            }
            if a.id.is_none() {
                let e = &a.expect;
                let resolved = if create && e.version == MATCH_ANY {
                    MATCH_DELETED
                } else {
                    e.version
                };
                if e.version_type != VersionType::Internal
                    || (resolved != MATCH_DELETED && resolved != MATCH_ANY)
                {
                    errors.insert(
                        before,
                        "an id must be provided if version type or value are set".into(),
                    );
                }
            }
            if let Some(id) = &a.id
                && id.len() > MAX_ID_BYTES
            {
                // ALLOC: one message; refused request.
                errors.push(format!(
                    "id [{id}] is too long, must be no longer than 512 bytes but was: {}",
                    id.len()
                ));
            }
            if a.pipeline.as_deref() == Some("") {
                errors.push("pipeline cannot be an empty string".into());
            }
        }
    }
}

/// Every failed check of `ops`, in request order; `no requests added` for
/// none.
pub fn errors(ops: &[Op]) -> Vec<String> {
    // ALLOC: the failed checks; a refused request.
    let mut errors = Vec::new();
    if ops.is_empty() {
        errors.push("no requests added".into());
    }
    for op in ops {
        op_errors(op, &mut errors);
    }
    errors
}
