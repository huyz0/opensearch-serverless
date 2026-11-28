//! What each action asks of the backend, and what it leaves in the record.

use serde_json::Value;

use super::backend::Backend;

/// Runs `action` (named `name`) on `index` and returns the step it took and
/// how it went: the success message, or the reason it failed.
pub(super) fn perform(
    name: &str,
    action: &Value,
    index: &str,
    backend: &dyn Backend,
) -> (&'static str, Result<String, String>) {
    match name {
        "read_only" => (
            "set_read_only",
            backend
                .set_read_only(index, true)
                .map(|()| format!("Successfully set index to read-only [index={index}]")),
        ),
        "read_write" => (
            "set_read_write",
            backend
                .set_read_only(index, false)
                .map(|()| format!("Successfully set index to read-write [index={index}]")),
        ),
        "delete" => (
            "attempt_delete",
            backend
                .delete(index)
                .map(|()| format!("Successfully deleted index [index={index}]")),
        ),
        "replica_count" => (
            "attempt_set_replica_count",
            replicas(action, index, backend),
        ),
        _ => (
            "",
            Err(format!("action [{name}] cannot run on this node yet")),
        ),
    }
}

fn replicas(action: &Value, index: &str, backend: &dyn Backend) -> Result<String, String> {
    let n = action["replica_count"]["number_of_replicas"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| "the action has no number_of_replicas".to_owned())?;
    backend
        .set_replicas(index, n)
        .map(|()| format!("Successfully updated number_of_replicas to {n} [index={index}]"))
}
