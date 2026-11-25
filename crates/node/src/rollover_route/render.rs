//! The body of a rollover answer, as core renders it.

use std::fmt::Write as _;

/// `s` as a JSON string.
fn quoted(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len().saturating_add(2));
    // Writing to a Vec cannot fail.
    let _ = oss_rest::write_json_str(&mut out, s);
    String::from_utf8_lossy(&out).into_owned()
}

/// The answer to a rollover: acknowledged only when it rolled over.
pub(super) fn response(
    old: &str,
    new: &str,
    conditions: &[(String, bool)],
    dry_run: bool,
    rolled_over: bool,
) -> String {
    // ALLOC: one small body, bounded by the names and conditions.
    let mut out = String::new();
    let _ = write!(
        out,
        "{{\"acknowledged\":{rolled_over},\"shards_acknowledged\":{rolled_over},\"old_index\":{},\"new_index\":{},\"rolled_over\":{rolled_over},\"dry_run\":{dry_run},\"conditions\":{{",
        quoted(old),
        quoted(new)
    );
    for (n, (key, met)) in conditions.iter().enumerate() {
        if n > 0 {
            out.push(',');
        }
        let _ = write!(out, "{}:{met}", quoted(key));
    }
    out.push_str("}}");
    out
}
