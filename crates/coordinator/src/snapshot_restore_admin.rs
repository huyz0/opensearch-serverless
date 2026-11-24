//! What a snapshot restore decides: its request body, the target names and
//! how each way it can fail is answered.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/snapshot/`
//! (`SnapshotHandler`'s restore branch, `SnapshotRequestBodies` and
//! `SnapshotOperations.restore`'s name resolution), shallow snapshots only
//! (ADR 0045).

use std::fmt::Write;

use oss_doc::mapping::json::{Json, parse, write_str};
use regex::Regex;

use crate::indices::admin::check_name as check_index_name;
use crate::route::resolve::Refusal;
use crate::snapshot_admin::check_name;
use crate::snapshot_take_admin::{MAX_TAKE_INDICES, too_large};

/// What a restore reads; everything else the Java accepts is refused with
/// its reason, below.
const KNOWN: [&str; 3] = ["indices", "rename_pattern", "rename_replacement"];

/// Core's other restore fields, each with why it is refused: the shell's
/// `RESTORE_REFUSED_KEYS`.
const REFUSED: [(&str, &str); 15] = [
    (
        "partial",
        "a restore here creates every shard of an index or refuses; there is no partial index to make",
    ),
    (
        "include_global_state",
        "there is no global state in a snapshot here to restore",
    ),
    (
        "include_aliases",
        "aliases are not captured in a snapshot here",
    ),
    (
        "rename_alias_pattern",
        "aliases are not captured in a snapshot here",
    ),
    (
        "rename_alias_replacement",
        "aliases are not captured in a snapshot here",
    ),
    (
        "index_settings",
        "the settings an index had when it was captured are restored with it, unchanged; change them on the restored index with PUT /{index}/_settings",
    ),
    (
        "ignore_index_settings",
        "the settings an index had when it was captured are restored with it, unchanged; change them on the restored index with PUT /{index}/_settings",
    ),
    (
        "settings",
        "repository settings are fixed when the repository is registered",
    ),
    (
        "storage_type",
        "a restored index is served from this deployment's own object store; there is no other storage type",
    ),
    (
        "source_remote_store_repository",
        "there is one object store here, and it is the repository",
    ),
    (
        "source_remote_translog_repository",
        "there is one object store here, and it is the repository",
    ),
    (
        "alias_write_index_policy",
        "aliases are not captured in a snapshot here",
    ),
    (
        "ignore_unavailable",
        "a restore names the indices it wants; one the snapshot did not capture is a 404",
    ),
    (
        "allow_no_indices",
        "a restore names the indices it wants; patterns are not matched against the snapshot",
    ),
    (
        "expand_wildcards",
        "a restore names the indices it wants; patterns are not matched against the snapshot",
    ),
];

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
        // ALLOC: none held; an empty name.
        Json::Arr(_) | Json::Obj(_) => String::new(),
    }
}

/// How target names are made from source names.
#[derive(Debug, Clone)]
pub struct Rename {
    pattern: Regex,
    replacement: String,
}

impl Rename {
    /// The target name for `source`: every match of the pattern replaced.
    #[must_use]
    pub fn apply(&self, source: &str) -> String {
        self.pattern
            .replace_all(source, self.replacement.as_str())
            .into_owned()
    }
}

/// What a restore was asked for.
#[derive(Debug, Clone)]
pub struct RestoreRequest {
    /// The indices to restore, in the order given and without repeats;
    /// `None` is every index the snapshot captured.
    pub indices: Option<Vec<String>>,
    /// The rename, when both its pattern and replacement were given.
    pub rename: Option<Rename>,
}

fn illegal(reason: String) -> Refusal {
    refuse(400, "illegal_argument_exception", reason)
}

/// Java's `Matcher.replaceAll` replacement, in the `regex` crate's: `$n` and
/// `${name}` name a group, `\x` is `x`, and any other `$` or a trailing `\`
/// is refused as Java refuses it.
fn replacement(java: &str, pattern: &Regex) -> Result<String, Refusal> {
    let groups = pattern.captures_len().saturating_sub(1);
    let mut out = String::with_capacity(java.len().saturating_add(8));
    let mut chars = java.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('$') => out.push_str("$$"),
                Some(next) => out.push(next),
                None => {
                    return Err(illegal(
                        "character to be escaped is missing in rename_replacement".to_owned(),
                    ));
                }
            },
            '$' => match chars.peek().copied() {
                Some('{') => {
                    chars.next();
                    let mut name = String::new();
                    loop {
                        match chars.next() {
                            Some('}') => break,
                            Some(n) => name.push(n),
                            None => {
                                return Err(illegal(
                                    "named capturing group is missing trailing '}' in rename_replacement"
                                        .to_owned(),
                                ));
                            }
                        }
                    }
                    if !pattern.capture_names().flatten().any(|n| n == name) {
                        return Err(illegal(format!(
                            "no group with name {{{name}}} in rename_pattern"
                        )));
                    }
                    let _ = write!(out, "${{{name}}}");
                }
                Some(d) if d.is_ascii_digit() => {
                    chars.next();
                    let mut number =
                        usize::from(u8::try_from(d).unwrap_or(b'0').saturating_sub(b'0'));
                    if number > groups {
                        return Err(illegal(format!("No group {number} in rename_pattern")));
                    }
                    // More digits are taken while the group still exists.
                    while let Some(next) = chars.peek().copied().filter(char::is_ascii_digit) {
                        let more = number.saturating_mul(10).saturating_add(usize::from(
                            u8::try_from(next).unwrap_or(b'0').saturating_sub(b'0'),
                        ));
                        if more > groups {
                            break;
                        }
                        number = more;
                        chars.next();
                    }
                    let _ = write!(out, "${{{number}}}");
                }
                _ => {
                    return Err(illegal(
                        "Illegal group reference in rename_replacement".to_owned(),
                    ));
                }
            },
            other => out.push(other),
        }
    }
    Ok(out)
}

/// The request of `POST /_snapshot/{repo}/{name}/_restore`, in the shell's
/// order: the names, the keys it refuses, the keys it does not know, then
/// the rename.
///
/// # Errors
///
/// 400 `invalid_snapshot_name_exception`; 400 `malformed_body` for a body
/// that is not an object; 501 `unsupported_restore` for a key the shell
/// refuses; 400 `unknown_parameter`; 400 `illegal_argument_exception` for a
/// rename pattern or replacement this node cannot apply.
pub fn request(repo: &str, name: &str, body: Option<&[u8]>) -> Result<RestoreRequest, Refusal> {
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
    for (key, why) in REFUSED {
        if members.iter().any(|(k, _)| k == key) {
            return Err(refuse(
                501,
                "unsupported_restore",
                // ALLOC: error path; one message.
                format!("{key} is not supported: {why}"),
            ));
        }
    }
    if let Some((key, _)) = members.iter().find(|(k, _)| !KNOWN.contains(&k.as_str())) {
        return Err(refuse(
            400,
            "unknown_parameter",
            // ALLOC: error path; one message.
            format!("Unknown parameter {key}"),
        ));
    }
    let get = |key: &str| {
        members
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
            .filter(|v| !matches!(v, Json::Null))
    };
    let indices = get("indices").map(|field| {
        // ALLOC: one name each, bounded by the 1 MiB body.
        let mut indices: Vec<String> = Vec::new();
        let mut push = |name: String| {
            if !indices.contains(&name) {
                indices.push(name);
            }
        };
        match field {
            Json::Str(list) => list
                .split(',')
                .filter(|piece| !piece.trim().is_empty())
                .for_each(|piece| push(piece.trim().to_owned())),
            Json::Arr(items) => items.iter().for_each(|item| push(text(item))),
            _ => {}
        }
        indices
    });
    let rename = match (get("rename_pattern"), get("rename_replacement")) {
        (Some(pattern), Some(with)) => {
            let source = text(pattern);
            let compiled = Regex::new(&source).map_err(|e| {
                illegal(format!(
                    "rename_pattern [{source}] is not a pattern this node can apply (no \
                     backreferences or lookaround): {e}"
                ))
            })?;
            let replacement = replacement(&text(with), &compiled)?;
            Some(Rename {
                pattern: compiled,
                replacement,
            })
        }
        _ => None,
    };
    Ok(RestoreRequest { indices, rename })
}

/// One index to restore: what the snapshot calls it and what it becomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The captured index's name.
    pub source: String,
    /// The restored index's name.
    pub target: String,
}

/// Resolves every target name before anything is written, as the shell does:
/// each requested index must be captured, no two may become one name, and
/// no target may be a reserved name (the node checks the rest of a name's
/// rules against the metadata plane's).
///
/// # Errors
///
/// 404 `index_not_found` for an index the snapshot did not capture; 400
/// `index_already_exists` for two sources with one target; 400
/// `snapshot_too_large` for more than [`MAX_TAKE_INDICES`] indices; the
/// index-name refusal for a target that is no name.
pub fn plan(request: &RestoreRequest, captured: &[String]) -> Result<Vec<Target>, Refusal> {
    let requested: Vec<&String> = match &request.indices {
        Some(named) => named.iter().collect(),
        None => captured.iter().collect(),
    };
    if requested.len() > MAX_TAKE_INDICES {
        // ALLOC: error path; one message.
        return Err(too_large(&format!(
            "a restore names at most {MAX_TAKE_INDICES} indices; this one names {}",
            requested.len()
        )));
    }
    for name in &requested {
        if !captured.contains(name) {
            return Err(index_not_captured(name));
        }
    }
    // ALLOC: one entry an index, at most MAX_TAKE_INDICES.
    let mut targets: Vec<Target> = Vec::with_capacity(requested.len());
    for source in requested {
        let target = request
            .rename
            .as_ref()
            .map_or_else(|| source.clone(), |r| r.apply(source));
        if let Some(first) = targets.iter().find(|t| t.target == target) {
            return Err(refuse(
                400,
                "index_already_exists",
                // ALLOC: error path; one message.
                format!(
                    "indices [{}] and [{source}] would both be restored as [{target}]",
                    first.source
                ),
            ));
        }
        check_index_name(&target, true)?;
        targets.push(Target {
            source: source.clone(),
            target,
        });
    }
    Ok(targets)
}

/// The refusal for an index the snapshot did not capture.
#[must_use]
pub fn index_not_captured(index: &str) -> Refusal {
    refuse(
        404,
        "index_not_found",
        // ALLOC: error path; one message.
        format!("index [{index}] was not captured in the snapshot"),
    )
}

/// The refusal for a target that already names an index or alias.
#[must_use]
pub fn target_exists(target: &str) -> Refusal {
    refuse(
        400,
        "index_already_exists",
        // ALLOC: error path; one message.
        format!("[{target}] already exists"),
    )
}

/// The refusal for a snapshot still being taken.
#[must_use]
pub fn in_progress(repo: &str, name: &str) -> Refusal {
    refuse(
        409,
        "snapshot_in_progress",
        // ALLOC: error path; one message.
        format!("snapshot {repo}/{name} is still being taken; there is nothing to restore yet"),
    )
}

/// The refusal for a shard that something else published on first.
#[must_use]
pub fn restore_conflict(index: &str, shard: u32) -> Refusal {
    refuse(
        409,
        "restore_conflict",
        // ALLOC: error path; one message.
        format!(
            "shard {shard} of [{index}] was written by something else while it was being \
             restored; the restore was rolled back and can be retried"
        ),
    )
}

/// The refusal for what a restore does not carry.
#[must_use]
pub fn unsupported_content(index: &str, why: &str) -> Refusal {
    refuse(
        501,
        "unsupported_snapshot_content",
        // ALLOC: error path; one message.
        format!("[{index}] cannot be restored here: {why}"),
    )
}

/// The refusal while restoring is switched off.
#[must_use]
pub fn restore_disabled() -> Refusal {
    refuse(
        501,
        "unsupported_operation",
        // ALLOC: error path; one message.
        "restoring snapshots is not enabled on this node (serverless.snapshots.restore.enabled)"
            .to_owned(),
    )
}

/// The answer of a restore that waited: the snapshot, the restored indices
/// and how many shards were copied.
#[must_use]
pub fn render_restored(snapshot: &str, targets: &[Target], shards: usize) -> String {
    // ALLOC: the answer, one entry an index.
    let mut out = String::from("{\"snapshot\":{\"snapshot\":");
    write_str(&mut out, snapshot);
    out.push_str(",\"indices\":[");
    for (i, t) in targets.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_str(&mut out, &t.target);
    }
    let _ = write!(
        out,
        "],\"shards\":{{\"total\":{shards},\"failed\":0,\"successful\":{shards}}}}}}}"
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(body: &str) -> Result<RestoreRequest, Refusal> {
        request("backups", "daily", Some(body.as_bytes()))
    }

    fn refused(body: &str) -> (u16, &'static str) {
        let r = ask(body).unwrap_err();
        (r.status, r.kind)
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn no_body_restores_everything_and_indices_come_as_a_list_or_a_string() {
        assert!(request("backups", "daily", None).unwrap().indices.is_none());
        assert!(ask("{}").unwrap().indices.is_none());
        assert_eq!(
            ask(r#"{"indices":["a","b","a"]}"#).unwrap().indices,
            Some(names(&["a", "b"]))
        );
        assert_eq!(
            ask(r#"{"indices":" a, b ,,a "}"#).unwrap().indices,
            Some(names(&["a", "b"]))
        );
        assert_eq!(ask(r#"{"indices":""}"#).unwrap().indices, Some(Vec::new()));
    }

    #[test]
    fn what_the_shell_refuses_is_refused_with_its_reason() {
        for key in REFUSED.map(|(k, _)| k) {
            let (status, kind) = refused(&format!(r#"{{"{key}":true}}"#));
            assert_eq!((status, kind), (501, "unsupported_restore"), "{key}");
        }
        let r = ask(r#"{"partial":true}"#).unwrap_err();
        assert!(
            r.reason.starts_with("partial is not supported: "),
            "{}",
            r.reason
        );
        assert_eq!(refused(r#"{"nope":1}"#), (400, "unknown_parameter"));
        assert_eq!(refused("[]"), (400, "malformed_body"));
        assert_eq!(refused("{"), (400, "malformed_body"));
        assert_eq!(
            request("_bad", "daily", None).unwrap_err().kind,
            "invalid_snapshot_name_exception"
        );
        assert_eq!(
            request("backups", "a,b", None).unwrap_err().kind,
            "invalid_snapshot_name_exception"
        );
    }

    #[test]
    fn a_rename_needs_both_halves_and_replaces_every_match() {
        assert!(ask(r#"{"rename_pattern":"a"}"#).unwrap().rename.is_none());
        assert!(
            ask(r#"{"rename_replacement":"b"}"#)
                .unwrap()
                .rename
                .is_none()
        );
        let r =
            ask(r#"{"rename_pattern":"logs-(.+)","rename_replacement":"restored-$1"}"#).unwrap();
        let rename = r.rename.unwrap();
        assert_eq!(rename.apply("logs-2026"), "restored-2026");
        assert_eq!(rename.apply("metrics"), "metrics");
        let r = ask(r#"{"rename_pattern":"o","rename_replacement":"0"}"#).unwrap();
        assert_eq!(r.rename.unwrap().apply("foo"), "f00");
    }

    #[test]
    fn replacements_follow_javas_rules() {
        let apply = |pattern: &str, with: &str, name: &str| {
            let body = format!(r#"{{"rename_pattern":{pattern:?},"rename_replacement":{with:?}}}"#);
            ask(&body).map(|r| r.rename.unwrap().apply(name))
        };
        // Digits are taken while the group exists: $1 then a literal 0.
        assert_eq!(apply("(a)", "$10", "a").unwrap(), "a0");
        assert_eq!(
            apply("(a)(b)(c)(d)(e)(f)(g)(h)(i)(j)", "$10", "abcdefghij").unwrap(),
            "j"
        );
        assert_eq!(apply("(?P<x>a)", "<${x}>", "a").unwrap(), "<a>");
        assert_eq!(apply("a", r"\$", "a").unwrap(), "$");
        assert_eq!(apply("a", r"\n", "a").unwrap(), "n");
        for bad in ["$", "$x", "$ ", "$2", "${nope}", "${open", r"\"] {
            let r = apply("(a)", bad, "a").unwrap_err();
            assert_eq!(
                (r.status, r.kind),
                (400, "illegal_argument_exception"),
                "{bad}"
            );
        }
        // Lookaround and backreferences cannot be applied.
        for bad in ["(?=a)a", r"(a)\1", "("] {
            let r = apply(bad, "x", "a").unwrap_err();
            assert_eq!(
                (r.status, r.kind),
                (400, "illegal_argument_exception"),
                "{bad}"
            );
        }
    }

    #[test]
    fn targets_are_resolved_whole_before_anything_is_written() {
        let captured = names(&["a", "b"]);
        let all = RestoreRequest {
            indices: None,
            rename: None,
        };
        let got = plan(&all, &captured).unwrap();
        assert_eq!(
            got,
            [
                Target {
                    source: "a".into(),
                    target: "a".into()
                },
                Target {
                    source: "b".into(),
                    target: "b".into()
                }
            ]
        );
        let missing = RestoreRequest {
            indices: Some(names(&["a", "zzz"])),
            rename: None,
        };
        let r = plan(&missing, &captured).unwrap_err();
        assert_eq!((r.status, r.kind), (404, "index_not_found"));
        let collide = ask(r#"{"rename_pattern":".","rename_replacement":"x"}"#).unwrap();
        let r = plan(&collide, &captured).unwrap_err();
        assert_eq!((r.status, r.kind), (400, "index_already_exists"));
        let reserved = ask(r#"{"rename_pattern":"a","rename_replacement":"_a"}"#).unwrap();
        assert_eq!(plan(&reserved, &captured).unwrap_err().status, 501);
        let many: Vec<String> = (0..=MAX_TAKE_INDICES).map(|i| format!("i{i}")).collect();
        let big = RestoreRequest {
            indices: None,
            rename: None,
        };
        let exactly: Vec<String> = many.iter().take(MAX_TAKE_INDICES).cloned().collect();
        assert_eq!(plan(&big, &exactly).unwrap().len(), MAX_TAKE_INDICES);
        let r = plan(&big, &many).unwrap_err();
        assert_eq!(r.kind, "snapshot_too_large");
    }

    #[test]
    fn each_answer_is_the_shells() {
        for (r, status, kind) in [
            (index_not_captured("i"), 404, "index_not_found"),
            (target_exists("i"), 400, "index_already_exists"),
            (in_progress("r", "s"), 409, "snapshot_in_progress"),
            (restore_conflict("i", 2), 409, "restore_conflict"),
            (
                unsupported_content("i", "why"),
                501,
                "unsupported_snapshot_content",
            ),
            (restore_disabled(), 501, "unsupported_operation"),
        ] {
            assert_eq!((r.status, r.kind), (status, kind));
            assert!(!r.reason.is_empty());
        }
        assert!(restore_conflict("i", 2).reason.contains("shard 2 of [i]"));
        let targets = [
            Target {
                source: "a".into(),
                target: "b\"".into(),
            },
            Target {
                source: "c".into(),
                target: "d".into(),
            },
        ];
        assert_eq!(
            render_restored("s", &targets, 3),
            r#"{"snapshot":{"snapshot":"s","indices":["b\"","d"],"shards":{"total":3,"failed":0,"successful":3}}}"#
        );
    }
}
