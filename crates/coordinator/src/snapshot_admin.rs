//! What the snapshot read routes decide: which names are exact, which are
//! valid, and how a repository or snapshot is answered.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/snapshot/`
//! (`SnapshotHandler`, `SnapshotRequestBodies.invalidName`, `RepositoryHandler`
//! and `SnapshotOperations.writeSnapshotInfo`), read-only (ADR 0038). The
//! answers carry this node's version, the shell's (3.10.0).

use std::fmt::Write;

use oss_doc::mapping::json::write_str;
use oss_format::{RepositoryRecord, SnapshotSummary};

use crate::route::resolve::Refusal;

/// The version the answers report: the shell's `Version.CURRENT`.
const VERSION_ID: i32 = 3_100_099;
const VERSION: &str = "3.10.0";

/// The most bytes a name may be.
const MAX_NAME_BYTES: usize = 255;

fn refuse(status: u16, kind: &'static str, reason: String) -> Refusal {
    Refusal {
        status,
        kind,
        reason,
    }
}

/// Whether `name` is not one exact name: `_all`, `_current`, a pattern or a
/// list. The read slice answers these with 501.
#[must_use]
pub fn is_selector(name: &str) -> bool {
    name == "_all" || name == "_current" || name.contains(['*', ','])
}

/// The refusal for a name this slice does not serve.
#[must_use]
pub fn unsupported_selector(name: &str) -> Refusal {
    refuse(
        501,
        "unsupported_operation",
        // ALLOC: error path; one message.
        format!("[{name}] selects more than one name, which snapshot reads do not support yet"),
    )
}

/// The refusal for a write route.
#[must_use]
pub fn unsupported_write() -> Refusal {
    refuse(
        501,
        "unsupported_operation",
        // ALLOC: one short message, once a refused request.
        "snapshot and repository changes are not supported by this node yet: snapshots are read-only here".to_owned(),
    )
}

/// The shell's `Names.validateId`: a name that is not empty, at most 255
/// bytes, free of control characters and of `/ \ # * ? " < > | ,`, not `.`
/// or `..` and not starting with `_`.
///
/// # Errors
///
/// A 400 `invalid_snapshot_name_exception` in the shell's words.
pub fn check_name(name: &str, what: &str) -> Result<(), Refusal> {
    let bad = |why: String| {
        Err(refuse(
            400,
            "invalid_snapshot_name_exception",
            // ALLOC: error path; one message.
            format!("Invalid {what} name [{name}], {why}"),
        ))
    };
    if name.is_empty() {
        // ALLOC: one short message, once a refused request.
        return bad("must not be empty".to_owned());
    }
    if name.len() > MAX_NAME_BYTES {
        // ALLOC: one short message, once a refused request.
        return bad(format!(
            "name is too long: ({} > {MAX_NAME_BYTES})",
            name.len()
        ));
    }
    if name.chars().any(char::is_control) {
        // ALLOC: one short message, once a refused request.
        return bad("must not contain control characters".to_owned());
    }
    if let Some(c) = name.chars().find(|c| {
        matches!(
            c,
            '/' | '\\' | '#' | '*' | '?' | '"' | '<' | '>' | '|' | ','
        )
    }) {
        // ALLOC: one short message, once a refused request.
        return bad(format!("must not contain '{c}'"));
    }
    if name == "." || name == ".." {
        // ALLOC: one short message, once a refused request.
        return bad("must not be '.' or '..'".to_owned());
    }
    if name.starts_with('_') {
        // ALLOC: one short message, once a refused request.
        return bad("must not start with '_'".to_owned());
    }
    Ok(())
}

/// The 404 for a repository that is not registered.
#[must_use]
pub fn repository_missing(name: &str) -> Refusal {
    refuse(
        404,
        "repository_missing_exception",
        // ALLOC: error path; one message.
        format!("[{name}] missing"),
    )
}

/// The 404 for a snapshot that does not exist.
#[must_use]
pub fn snapshot_missing(repo: &str, name: &str) -> Refusal {
    refuse(
        404,
        "snapshot_missing_exception",
        // ALLOC: error path; one message.
        format!("no such snapshot: {repo}/{name}"),
    )
}

/// `GET _snapshot[/{repo}]`: each repository's `type` and `settings`.
#[must_use]
pub fn render_repositories(records: &[RepositoryRecord]) -> String {
    // ALLOC: the answer, one entry a repository, at most the listing cap.
    let mut out = String::from("{");
    for (i, r) in records.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_str(&mut out, &r.name);
        out.push_str(":{\"type\":");
        write_str(&mut out, &r.repo_type);
        out.push_str(",\"settings\":{");
        for (j, (key, value)) in r.settings.iter().enumerate() {
            if j > 0 {
                out.push(',');
            }
            write_str(&mut out, key);
            out.push(':');
            write_str(&mut out, value);
        }
        out.push_str("}}");
    }
    out.push('}');
    out
}

/// `GET _snapshot/{repo}/{name}`: `{"snapshots":[...]}` as the shell's
/// `writeSnapshotInfo` writes one. The state comes from the record: an
/// index with shards and none captured is a capture still running.
#[must_use]
pub fn render_snapshot(s: &SnapshotSummary) -> String {
    let captured: u64 = s.indices.iter().map(|i| u64::from(i.captured)).sum();
    // ALLOC: the answer, one entry an index.
    let mut out = String::from("{\"snapshots\":[{\"snapshot\":");
    write_str(&mut out, &s.name);
    out.push_str(",\"uuid\":");
    match &s.uuid {
        Some(uuid) => write_str(&mut out, uuid),
        None => out.push_str("null"),
    }
    let _ = write!(
        out,
        ",\"version_id\":{VERSION_ID},\"version\":\"{VERSION}\",\"remote_store_index_shallow_copy\":{},\"indices\":[",
        s.shallow
    );
    for (i, index) in s.indices.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_str(&mut out, &index.name);
    }
    let _ = write!(
        out,
        "],\"data_streams\":[],\"include_global_state\":true,\"state\":\"{}\",\"start_time\":\"{}\",\"start_time_in_millis\":{},\"end_time\":\"{}\",\"end_time_in_millis\":{},\"duration_in_millis\":{},\"failures\":[],\"shards\":{{\"total\":{captured},\"failed\":0,\"successful\":{captured}}}}}]}}",
        if s.in_progress() {
            "IN_PROGRESS"
        } else {
            "SUCCESS"
        },
        instant(s.start_millis),
        s.start_millis,
        instant(s.end_millis),
        s.end_millis,
        s.end_millis.saturating_sub(s.start_millis),
    );
    out
}

/// `java.time.Instant.toString` of a millisecond time: `2023-11-14T22:13:20Z`,
/// with `.SSS` when the millis are not zero.
// ARITH: civil-date arithmetic over a bounded day count; values stay far inside i64.
#[allow(clippy::arithmetic_side_effects)]
fn instant(millis: i64) -> String {
    let days = millis.div_euclid(86_400_000);
    let ms_of_day = millis.rem_euclid(86_400_000);
    // Howard Hinnant's civil-from-days.
    let z = days.saturating_add(719_468);
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    let (h, m, s, ms) = (
        ms_of_day / 3_600_000,
        ms_of_day / 60_000 % 60,
        ms_of_day / 1000 % 60,
        ms_of_day % 1000,
    );
    if ms == 0 {
        // ALLOC: one timestamp, a snapshot's field.
        format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z")
    } else {
        // ALLOC: one timestamp, a snapshot's field.
        format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}.{ms:03}Z")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oss_format::SnapshotIndex;

    #[test]
    fn selectors_are_not_exact_names() {
        for s in ["_all", "_current", "a*", "a,b", "*"] {
            assert!(is_selector(s), "{s}");
        }
        for s in ["daily-1", "a.b", "_x"] {
            assert!(!is_selector(s), "{s}");
        }
        assert_eq!(unsupported_selector("_all").status, 501);
        assert_eq!(unsupported_write().status, 501);
    }

    #[test]
    fn names_are_checked_as_the_shell_checks_them() {
        assert!(check_name("daily-1", "snapshot").is_ok());
        assert!(check_name("A.b_c", "repository").is_ok());
        for (name, why) in [
            ("", "must not be empty"),
            ("a#b", "must not contain '#'"),
            ("a/b", "must not contain '/'"),
            ("a b\u{7}", "must not contain control characters"),
            (".", "must not be '.' or '..'"),
            ("..", "must not be '.' or '..'"),
            ("_x", "must not start with '_'"),
        ] {
            let r = check_name(name, "snapshot").expect_err(name);
            assert_eq!((r.status, r.kind), (400, "invalid_snapshot_name_exception"));
            assert_eq!(r.reason, format!("Invalid snapshot name [{name}], {why}"));
        }
        let long = "a".repeat(256);
        assert!(
            check_name(&long, "snapshot")
                .unwrap_err()
                .reason
                .ends_with("(256 > 255)")
        );
        assert!(check_name(&"a".repeat(255), "snapshot").is_ok());
    }

    #[test]
    fn a_repository_is_answered_by_name_with_type_and_settings() {
        let r = |name: &str| RepositoryRecord {
            name: name.to_owned(),
            repo_type: "fs".to_owned(),
            created_at_millis: 1,
            settings: vec![("location".to_owned(), "x\"y".to_owned())],
        };
        assert_eq!(
            render_repositories(&[r("a"), r("b")]),
            r#"{"a":{"type":"fs","settings":{"location":"x\"y"}},"b":{"type":"fs","settings":{"location":"x\"y"}}}"#
        );
        assert_eq!(render_repositories(&[]), "{}");
    }

    fn summary(captured: u32) -> SnapshotSummary {
        SnapshotSummary {
            repo: "r".into(),
            name: "s".into(),
            uuid: Some("u".into()),
            storage_generation: None,
            shallow: false,
            start_millis: 1_700_000_001_000,
            end_millis: 1_700_000_002_345,
            indices: vec![SnapshotIndex {
                name: "logs".into(),
                uuid: "A".into(),
                shards: 2,
                captured,
            }],
        }
    }

    #[test]
    fn a_snapshot_is_answered_in_the_shells_shape() {
        assert_eq!(
            render_snapshot(&summary(2)),
            r#"{"snapshots":[{"snapshot":"s","uuid":"u","version_id":3100099,"version":"3.10.0","remote_store_index_shallow_copy":false,"indices":["logs"],"data_streams":[],"include_global_state":true,"state":"SUCCESS","start_time":"2023-11-14T22:13:21Z","start_time_in_millis":1700000001000,"end_time":"2023-11-14T22:13:22.345Z","end_time_in_millis":1700000002345,"duration_in_millis":1345,"failures":[],"shards":{"total":2,"failed":0,"successful":2}}]}"#
        );
    }

    #[test]
    fn a_capture_still_running_is_in_progress() {
        let out = render_snapshot(&summary(0));
        assert!(out.contains(r#""state":"IN_PROGRESS""#), "{out}");
        assert!(
            out.contains(r#""shards":{"total":0,"failed":0,"successful":0}"#),
            "{out}"
        );
    }

    #[test]
    fn instants_print_as_java_prints_them() {
        assert_eq!(instant(0), "1970-01-01T00:00:00Z");
        assert_eq!(instant(951_782_400_000), "2000-02-29T00:00:00Z");
        assert_eq!(instant(-1), "1969-12-31T23:59:59.999Z");
        assert_eq!(instant(1_700_000_000_000), "2023-11-14T22:13:20Z");
    }
}
