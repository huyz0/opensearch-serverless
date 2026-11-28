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
        "alias" => ("attempt_alias", aliases(action, index, backend)),
        "notification" => ("attempt_notification", notification(action, index, backend)),
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

/// The aliases an `add` or `remove` entry names: `alias` or `aliases`.
fn alias_names(entry: &Value) -> Vec<&str> {
    let mut names: Vec<&str> = entry["alias"].as_str().into_iter().collect();
    if let Some(list) = entry["aliases"].as_array() {
        names.extend(list.iter().filter_map(Value::as_str));
    }
    names
}

fn aliases(action: &Value, index: &str, backend: &dyn Backend) -> Result<String, String> {
    let entries = action["alias"]["actions"]
        .as_array()
        .filter(|a| !a.is_empty())
        .ok_or_else(|| "the action has no alias actions".to_owned())?;
    for entry in entries {
        let (add, body) = match (entry.get("add"), entry.get("remove")) {
            (Some(body), None) => (true, body),
            (None, Some(body)) => (false, body),
            _ => return Err("an alias action is an add or a remove".to_owned()),
        };
        let write = body["is_write_index"].as_bool();
        let names = alias_names(body);
        if names.is_empty() {
            return Err("an alias action names no alias".to_owned());
        }
        for name in names {
            backend.set_alias(index, add, name, write)?;
        }
    }
    Ok(format!("Successfully updated aliases [index={index}]"))
}

fn notification(action: &Value, index: &str, backend: &dyn Backend) -> Result<String, String> {
    let body = &action["notification"];
    let channel = body["channel"]["id"]
        .as_str()
        .filter(|c| !c.is_empty())
        .ok_or_else(|| "the action has no channel id".to_owned())?;
    // The one variable of the plugin's template a serverless node fills in.
    let text = body["message_template"]["source"]
        .as_str()
        .unwrap_or("Notification")
        .replace("{{ctx.index}}", index);
    backend
        .notify(channel, &text)
        .map(|()| format!("Successfully sent notification [index={index}]"))
}
