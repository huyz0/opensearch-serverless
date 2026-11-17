//! What a primary does with an index or delete: `IndexingStrategyPlanner`
//! and `DeletionStrategyPlanner`'s `planOperationAsPrimary`, given the
//! current version of the id.
//!
//! Ported from `IndexingStrategyPlanner`, `DeletionStrategyPlanner` and
//! `VersionConflictEngineException` (server, OpenSearch 3.10).

use super::version::{NOT_FOUND, VersionType};

/// `SequenceNumbers.UNASSIGNED_SEQ_NO`.
pub const UNASSIGNED_SEQ_NO: i64 = -2;

/// What a document's id currently holds: `VersionValue`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Current {
    /// Its version.
    pub version: i64,
    /// The sequence number that wrote it.
    pub seq_no: i64,
    /// The primary term that wrote it.
    pub term: i64,
    /// Whether that write was a delete (a tombstone).
    pub deleted: bool,
}

/// The concurrency conditions a request sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Expect {
    /// `version`, or [`super::version::MATCH_ANY`].
    pub version: i64,
    /// `version_type`.
    pub version_type: VersionType,
    /// `if_seq_no`, or [`UNASSIGNED_SEQ_NO`].
    pub if_seq_no: i64,
    /// `if_primary_term`, or 0.
    pub if_primary_term: i64,
}

/// A write the planner lets through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    /// The version the write takes.
    pub version: i64,
    /// Whether the id held no live document: an index creates, a delete
    /// finds nothing.
    pub not_found_or_deleted: bool,
}

/// `VersionConflictEngineException`'s message for `id`.
fn conflict(id: &str, explanation: &str) -> String {
    // ALLOC: one message; refused write.
    format!("[{id}]: version conflict, {explanation}")
}

/// Plans an index or a delete of `id` as a primary: the version it takes,
/// or the version conflict's message. Both planners share these checks in
/// this order.
///
/// # Errors
///
/// The `VersionConflictEngineException` message when `if_seq_no` and
/// `if_primary_term` do not name the current document, or the version
/// type finds a conflict.
pub fn plan(id: &str, current: Option<Current>, expect: &Expect) -> Result<Plan, String> {
    let (version, gone) = current.map_or((NOT_FOUND, true), |c| (c.version, c.deleted));
    if expect.if_seq_no != UNASSIGNED_SEQ_NO {
        let found = current.filter(|_| !gone);
        let matches =
            found.is_some_and(|c| c.seq_no == expect.if_seq_no && c.term == expect.if_primary_term);
        if !matches {
            let tail = match found {
                // ALLOC: one message; refused write.
                Some(c) => format!(
                    " current document has seqNo [{}] and primary term [{}]",
                    c.seq_no, c.term
                ),
                // ALLOC: one message; refused write.
                None => " but no document was found".to_owned(),
            };
            // ALLOC: one message; refused write.
            let explanation = format!(
                "required seqNo [{}], primary term [{}].{tail}",
                expect.if_seq_no, expect.if_primary_term
            );
            return Err(conflict(id, &explanation));
        }
    }
    if expect.version_type.conflicts(version, expect.version, gone) {
        return Err(conflict(
            id,
            &expect.version_type.explain(version, expect.version),
        ));
    }
    Ok(Plan {
        version: expect.version_type.next(version, expect.version),
        not_found_or_deleted: gone,
    })
}

/// `IndexRequest.validate` and `DeleteRequest.validate`'s version checks
/// (`validateSeqNoBasedCASParams`), `create` for `op_type=create`.
///
/// # Errors
///
/// `action_request_validation_exception`'s message, every failed check
/// numbered as `ActionRequestValidationException` lists them.
pub fn validate(expect: &Expect, create: bool) -> Result<(), String> {
    // ALLOC: the failed checks, at most five; refused request.
    let mut errors: Vec<String> = Vec::new();
    version_errors(expect, create, &mut errors);
    if errors.is_empty() {
        return Ok(());
    }
    Err(validation_failed(&errors))
}

/// `ActionRequestValidationException`'s message over `errors`, each
/// numbered from 1.
pub fn validation_failed(errors: &[String]) -> String {
    // ALLOC: one message; refused request.
    let mut message = String::from("Validation Failed: ");
    for (i, e) in errors.iter().enumerate() {
        message.push_str(&format!("{}: {e};", i.saturating_add(1)));
    }
    message
}

/// Adds to `errors` the checks `validate` makes, in the Java's order.
pub fn version_errors(expect: &Expect, create: bool, errors: &mut Vec<String>) {
    use super::version::{MATCH_ANY, MATCH_DELETED};
    if create {
        if expect.version_type != VersionType::Internal {
            errors.push(
                "create operations only support internal versioning. use index instead".into(),
            );
        } else if expect.version != MATCH_ANY && expect.version != MATCH_DELETED {
            errors.push(
                "create operations do not support explicit versions. use index instead".into(),
            );
        } else if expect.if_seq_no != UNASSIGNED_SEQ_NO || expect.if_primary_term != 0 {
            errors
                .push("create operations do not support compare and set. use index instead".into());
        }
    } else {
        let (version, ty) = (expect.version, expect.version_type);
        let valid = match ty {
            VersionType::Internal => {
                version > 0 || version == MATCH_ANY || version == MATCH_DELETED
            }
            VersionType::External | VersionType::ExternalGte => version >= 0,
        };
        if !valid {
            let name = match ty {
                VersionType::Internal => "INTERNAL",
                VersionType::External => "EXTERNAL",
                VersionType::ExternalGte => "EXTERNAL_GTE",
            };
            // ALLOC: one message; refused request.
            errors.push(format!(
                "illegal version value [{version}] for version type [{name}]"
            ));
        }
        if ty == VersionType::Internal && version != MATCH_ANY && version != MATCH_DELETED {
            errors.push(
                "internal versioning can not be used for optimistic concurrency control. Please use `if_seq_no` and `if_primary_term` instead"
                    .into(),
            );
        }
        if expect.if_seq_no != UNASSIGNED_SEQ_NO
            && (ty != VersionType::Internal || version != MATCH_ANY)
        {
            errors.push("compare and write operations can not use versioning".into());
        }
        if expect.if_primary_term == 0 && expect.if_seq_no != UNASSIGNED_SEQ_NO {
            errors.push("ifSeqNo is set, but primary term is [0]".into());
        }
        if expect.if_primary_term != 0 && expect.if_seq_no == UNASSIGNED_SEQ_NO {
            // ALLOC: one message; refused request.
            errors.push(format!(
                "ifSeqNo is unassigned, but primary term is [{}]",
                expect.if_primary_term
            ));
        }
    }
}
