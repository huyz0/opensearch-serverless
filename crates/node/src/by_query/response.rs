//! The answer to a delete-by-query, as OpenSearch writes it.

use std::fmt::Write as _;

use oss_coordinator::docs::by_query::Walk;
use oss_doc::mapping::json::write_str;

/// The status and body for a finished walk: 200, or 409 when it aborted on
/// a conflict.
pub(super) fn render(walk: &Walk, index: &str, took_millis: u64) -> (u16, String) {
    // ALLOC: one small body a request, bounded by the id and message.
    let mut out = String::new();
    let _ = write!(
        out,
        r#"{{"took":{took_millis},"timed_out":false,"total":{},"deleted":{},"batches":{},"version_conflicts":{},"noops":{},"retries":{{"bulk":0,"search":0}},"throttled_millis":0,"requests_per_second":-1.0,"throttled_until_millis":0,"failures":["#,
        walk.visited, walk.changed, walk.batches, walk.conflicts, walk.noops,
    );
    let mut status = 200;
    if let (Some(id), Some(message)) = (&walk.aborted_on, &walk.conflict) {
        status = 409;
        out.push_str(r#"{"index":"#);
        write_str(&mut out, index);
        out.push_str(r#","id":"#);
        write_str(&mut out, id);
        out.push_str(r#","cause":{"type":"version_conflict_engine_exception","reason":"#);
        write_str(&mut out, message);
        out.push_str(r#"},"status":409}"#);
    }
    out.push_str("]}");
    (status, out)
}
