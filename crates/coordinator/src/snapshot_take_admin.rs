//! What a snapshot take decides: its request body, the bounds a take stays
//! within and how each way it can fail is answered.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/snapshot/`
//! (`SnapshotHandler`'s create branch, `SnapshotRequestBodies` and
//! `SnapshotOperations.handleCreate`), shallow snapshots only (ADR 0042).

use oss_doc::mapping::json::{Json, parse};

use crate::route::resolve::Refusal;
use crate::snapshot_admin::check_name;

/// The most indices one take names.
pub const MAX_TAKE_INDICES: usize = 100;
/// The most shards one take captures, over all its indices.
pub const MAX_TAKE_SHARDS: usize = 4_096;

/// What a take was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakeRequest {
    /// The index names, in the order given and without repeats.
    pub indices: Vec<String>,
    /// Skip a missing index instead of failing.
    pub ignore_unavailable: bool,
    /// Skip an index with a shard that published nothing.
    pub partial: bool,
    /// Capture the cluster's global state too; this node captures none.
    pub include_global_state: bool,
}

fn refuse(status: u16, kind: &'static str, reason: String) -> Refusal {
    Refusal {
        status,
        kind,
        reason,
    }
}

fn text(value: &Json) -> String {
    match value {
        Json::Str(s) | Json::Num(s) => s.clone(),
        // ALLOC: one name, bounded by the 1 MiB body.
        Json::Bool(b) => b.to_string(),
        // ALLOC: one short name, once a request.
        Json::Null => "null".to_owned(),
        // The Java's `String.valueOf` of a list or map: not a name.
        // ALLOC: none held; an empty name.
        Json::Arr(_) | Json::Obj(_) => String::new(),
    }
}

fn flag(value: Option<&Json>) -> bool {
    match value {
        None | Some(Json::Null) => false,
        Some(Json::Bool(b)) => *b,
        Some(other) => text(other).eq_ignore_ascii_case("true"),
    }
}

/// The request of `PUT`/`POST /_snapshot/{repo}/{name}`, in the shell's
/// order: the names, then the body.
///
/// # Errors
///
/// 400 `invalid_snapshot_name_exception`; 400 `malformed_body` for a body
/// that is not an object; 501 `unsupported_snapshot_option` for `metadata`;
/// 400 `unknown_parameter`; 400 `missing_indices`; 400 `snapshot_too_large`
/// for more than [`MAX_TAKE_INDICES`] indices.
pub fn request(repo: &str, name: &str, body: Option<&[u8]>) -> Result<TakeRequest, Refusal> {
    check_name(repo, "repository")?;
    check_name(name, "snapshot")?;
    let members = match body.filter(|b| !b.is_empty()) {
        // ALLOC: none held; an empty list.
        None => Vec::new(),
        Some(bytes) => match parse(bytes) {
            Ok(Json::Obj(members)) => members,
            _ => {
                return Err(refuse(
                    400,
                    "malformed_body",
                    // ALLOC: error path; one message.
                    "could not parse the snapshot request: the body is not a JSON object"
                        .to_owned(),
                ));
            }
        },
    };
    if members.iter().any(|(k, _)| k == "metadata") {
        return Err(refuse(
            501,
            "unsupported_snapshot_option",
            // ALLOC: error path; one message.
            "metadata is not supported: a snapshot record here carries no user metadata, so it \
             would be accepted and never returned"
                .to_owned(),
        ));
    }
    if let Some((key, _)) = members.iter().find(|(k, _)| {
        !matches!(
            k.as_str(),
            "indices" | "ignore_unavailable" | "partial" | "include_global_state"
        )
    }) {
        return Err(refuse(
            400,
            "unknown_parameter",
            // ALLOC: error path; one message.
            format!("Unknown parameter {key}"),
        ));
    }
    let get = |key: &str| members.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v);
    // ALLOC: one name each, bounded by the 1 MiB body.
    let mut indices: Vec<String> = Vec::new();
    let mut push = |name: String| {
        if !indices.contains(&name) {
            indices.push(name);
        }
    };
    match get("indices") {
        Some(Json::Str(list)) => list
            .split(',')
            .filter(|piece| !piece.trim().is_empty())
            // ALLOC: one name, bounded by the 1 MiB body.
            .for_each(|piece| push(piece.trim().to_owned())),
        Some(Json::Arr(items)) => items.iter().for_each(|item| push(text(item))),
        _ => {}
    }
    if indices.is_empty() {
        return Err(refuse(
            400,
            "missing_indices",
            // ALLOC: error path; one message.
            "a snapshot needs an 'indices' field naming what to capture; this surface does not \
             enumerate what exists"
                .to_owned(),
        ));
    }
    if indices.len() > MAX_TAKE_INDICES {
        // ALLOC: error path; one message.
        return Err(too_large(&format!(
            "a snapshot names at most {MAX_TAKE_INDICES} indices; this one names {}",
            indices.len()
        )));
    }
    Ok(TakeRequest {
        indices,
        ignore_unavailable: flag(get("ignore_unavailable")),
        partial: flag(get("partial")),
        include_global_state: flag(get("include_global_state")),
    })
}

/// The refusal for a take over its bounds.
#[must_use]
pub fn too_large(reason: &str) -> Refusal {
    // ALLOC: error path; one message.
    refuse(400, "snapshot_too_large", reason.to_owned())
}

/// Whether `shards` more shards keep a take within [`MAX_TAKE_SHARDS`].
///
/// # Errors
///
/// 400 `snapshot_too_large`.
pub fn check_shards(total: usize) -> Result<(), Refusal> {
    if total > MAX_TAKE_SHARDS {
        // ALLOC: error path; one message.
        return Err(too_large(&format!(
            "a snapshot captures at most {MAX_TAKE_SHARDS} shards; this one has {total}"
        )));
    }
    Ok(())
}

/// The refusal for a repository that copies data: only shallow snapshots are
/// taken.
#[must_use]
pub fn unsupported_storage(repo: &str) -> Refusal {
    refuse(
        501,
        "unsupported_snapshot_storage",
        // ALLOC: error path; one message.
        format!(
            "repository [{repo}] takes standard snapshots, which copy shard data; this node takes \
             only shallow snapshots, from a repository with remote_store_index_shallow_copy true"
        ),
    )
}

/// The refusal while taking is switched off.
#[must_use]
pub fn take_disabled() -> Refusal {
    refuse(
        501,
        "unsupported_operation",
        // ALLOC: error path; one message.
        "taking snapshots is not enabled on this node (serverless.snapshots.take.enabled)"
            .to_owned(),
    )
}

/// The refusal for a name that already holds a snapshot.
#[must_use]
pub fn already_exists(repo: &str, name: &str) -> Refusal {
    refuse(
        400,
        "snapshot_already_exists",
        // ALLOC: error path; one message.
        format!(
            "[{repo}:{name}] Invalid snapshot name [{name}], snapshot with the same name already exists"
        ),
    )
}

/// The refusal for a name too long to key.
#[must_use]
pub fn name_too_long(repo: &str, name: &str) -> Refusal {
    refuse(
        400,
        "invalid_snapshot_name_exception",
        // ALLOC: error path; one message.
        format!("[{repo}:{name}] the repository and snapshot names together are over 255 bytes"),
    )
}

/// The refusal for an index that is not there.
#[must_use]
pub fn index_not_found(index: &str) -> Refusal {
    refuse(
        404,
        "index_not_found",
        // ALLOC: error path; one message.
        format!("no such index [{index}]"),
    )
}

/// The refusal for a system index.
#[must_use]
pub fn system_index(index: &str) -> Refusal {
    refuse(
        403,
        "system_index",
        // ALLOC: error path; one message.
        format!("[{index}] is a system index and cannot be captured"),
    )
}

/// The refusal for an alias, which is captured index by index.
#[must_use]
pub fn is_alias(name: &str) -> Refusal {
    refuse(
        400,
        "illegal_argument_exception",
        // ALLOC: error path; one message.
        format!("[{name}] is an alias; name the indices it points to"),
    )
}

/// The refusal for an index whose shards are not all its slots.
#[must_use]
pub fn unsupported_layout(index: &str) -> Refusal {
    refuse(
        501,
        "unsupported_snapshot_layout",
        // ALLOC: error path; one message.
        format!("[{index}] has retired or pending shards, which a snapshot record cannot describe"),
    )
}

/// The refusal for an index with a shard that published nothing.
#[must_use]
pub fn nothing_published(index: &str, shard: u32) -> Refusal {
    refuse(
        409,
        "nothing_published",
        // ALLOC: error path; one message.
        format!("[{index}] shard {shard} has published nothing to capture yet"),
    )
}

/// The refusal for a take overtaken by another writer or a replaced index.
#[must_use]
pub fn conflict(reason: &str) -> Refusal {
    // ALLOC: error path; one message.
    refuse(409, "snapshot_conflict", reason.to_owned())
}

/// The refusal for a record over its size bound.
#[must_use]
pub fn record_too_large(needed: u64) -> Refusal {
    refuse(
        413,
        "snapshot_record_too_large",
        // ALLOC: error path; one message.
        format!("the snapshot record would be {needed} bytes, over the limit"),
    )
}

/// The answer of a take without `wait_for_completion`.
pub const ACCEPTED: &str = r#"{"accepted":true}"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(body: &str) -> Result<TakeRequest, Refusal> {
        request("backups", "daily", Some(body.as_bytes()))
    }

    fn refused(body: &str) -> (u16, &'static str) {
        let r = ask(body).unwrap_err();
        (r.status, r.kind)
    }

    #[test]
    fn indices_come_as_a_list_or_a_comma_string_without_repeats() {
        assert_eq!(
            ask(r#"{"indices":["a","b","a"]}"#).unwrap().indices,
            ["a", "b"]
        );
        assert_eq!(
            ask(r#"{"indices":" a, b ,,a "}"#).unwrap().indices,
            ["a", "b"]
        );
    }

    #[test]
    fn flags_read_as_booleans_or_their_text() {
        let r = ask(r#"{"indices":"a","ignore_unavailable":"TRUE","partial":true,"include_global_state":false}"#)
            .unwrap();
        assert!(r.ignore_unavailable && r.partial && !r.include_global_state);
        let r = ask(r#"{"indices":"a","ignore_unavailable":"no"}"#).unwrap();
        assert!(!r.ignore_unavailable && !r.partial && !r.include_global_state);
        let r = ask(r#"{"indices":"a","include_global_state":"true"}"#).unwrap();
        assert!(r.include_global_state);
    }

    #[test]
    fn each_refusal_has_the_shells_status_and_name() {
        assert_eq!(
            refused(r#"{"indices":"a","metadata":{}}"#),
            (501, "unsupported_snapshot_option")
        );
        assert_eq!(
            refused(r#"{"indices":"a","wait":1}"#),
            (400, "unknown_parameter")
        );
        assert_eq!(refused(r#"{"indices":""}"#), (400, "missing_indices"));
        assert_eq!(refused(r#"{"indices":[]}"#), (400, "missing_indices"));
        assert_eq!(refused("{}"), (400, "missing_indices"));
        assert_eq!(refused("[1]"), (400, "malformed_body"));
        assert_eq!(
            request("backups", "daily", None).unwrap_err().kind,
            "missing_indices"
        );
        assert_eq!(
            request("backups", "_x", Some(br#"{"indices":"a"}"#))
                .unwrap_err()
                .kind,
            "invalid_snapshot_name_exception"
        );
        assert_eq!(
            request("a#b", "daily", Some(br#"{"indices":"a"}"#))
                .unwrap_err()
                .kind,
            "invalid_snapshot_name_exception"
        );
    }

    #[test]
    fn the_index_bound_is_exact() {
        let names = |n: usize| {
            let list: Vec<String> = (0..n).map(|i| format!("\"i{i}\"")).collect();
            format!(r#"{{"indices":[{}]}}"#, list.join(","))
        };
        assert_eq!(ask(&names(MAX_TAKE_INDICES)).unwrap().indices.len(), 100);
        assert_eq!(
            refused(&names(MAX_TAKE_INDICES + 1)),
            (400, "snapshot_too_large")
        );
    }

    #[test]
    fn the_shard_bound_is_exact() {
        assert!(check_shards(MAX_TAKE_SHARDS).is_ok());
        assert_eq!(
            check_shards(MAX_TAKE_SHARDS + 1).unwrap_err().kind,
            "snapshot_too_large"
        );
    }

    #[test]
    fn the_failures_carry_their_status_and_name() {
        let all = [
            (
                unsupported_storage("r"),
                501,
                "unsupported_snapshot_storage",
            ),
            (take_disabled(), 501, "unsupported_operation"),
            (already_exists("r", "s"), 400, "snapshot_already_exists"),
            (
                name_too_long("r", "s"),
                400,
                "invalid_snapshot_name_exception",
            ),
            (index_not_found("i"), 404, "index_not_found"),
            (system_index("i"), 403, "system_index"),
            (is_alias("a"), 400, "illegal_argument_exception"),
            (unsupported_layout("i"), 501, "unsupported_snapshot_layout"),
            (nothing_published("i", 2), 409, "nothing_published"),
            (conflict("x"), 409, "snapshot_conflict"),
            (record_too_large(9), 413, "snapshot_record_too_large"),
        ];
        for (r, status, kind) in all {
            assert_eq!((r.status, r.kind), (status, kind), "{}", r.reason);
        }
        assert_eq!(ACCEPTED, r#"{"accepted":true}"#);
    }
}
