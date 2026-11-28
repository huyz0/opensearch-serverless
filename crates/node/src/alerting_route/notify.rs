//! The actions of a trigger: which are due on an ACTIVE alert (throttle
//! aside, every run), the message each renders to, and the seam they are sent
//! through.

use serde_json::{Value, json};

use crate::notifications_route::Notifier;

/// Sends a rendered message to a destination (a notification config id).
pub trait ActionSender: Send + Sync {
    /// Sends `text` to `destination`.
    ///
    /// # Errors
    ///
    /// Why the delivery failed, in a short word or two.
    fn send(&self, destination: &str, text: &str) -> Result<(), String>;
}

impl ActionSender for Notifier {
    fn send(&self, destination: &str, text: &str) -> Result<(), String> {
        let delivery = Notifier::send(self, destination, text);
        if (200..300).contains(&delivery.status_code) {
            Ok(())
        } else {
            Err(delivery.status_text)
        }
    }
}

/// One message to send for an alert's action.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Dispatch {
    pub(super) alert_id: String,
    pub(super) action_id: String,
    pub(super) action_name: String,
    pub(super) destination: String,
    pub(super) text: String,
}

/// The value a dotted path names in `root`; array items by index.
fn lookup<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(root, |v, part| match v {
        Value::Array(items) => items.get(part.parse::<usize>().ok()?),
        _ => v.get(part),
    })
}

/// `template` with each `{{path}}` replaced by the value `path` names in
/// `{"ctx": ctx}`; a path that names nothing is empty and a section tag is
/// left as written.
pub(super) fn render(template: &str, ctx: &Value) -> String {
    let root = json!({ "ctx": ctx });
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        let (head, tail) = rest.split_at(open);
        out.push_str(head);
        let Some(close) = tail.find("}}") else {
            rest = tail;
            break;
        };
        let inner = tail.get(2..close).unwrap_or_default().trim();
        if inner.starts_with(['#', '^', '/', '!', '>']) {
            out.push_str(tail.get(..close.saturating_add(2)).unwrap_or_default());
        } else {
            match lookup(&root, inner) {
                Some(Value::String(s)) => out.push_str(s),
                Some(Value::Null) | None => {}
                Some(other) => out.push_str(&other.to_string()),
            }
        }
        rest = tail.get(close.saturating_add(2)..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

/// The throttle window of an action in milliseconds, none when it has none.
fn throttle_ms(action: &Value) -> Option<i64> {
    if action["throttle_enabled"].as_bool() != Some(true) {
        return None;
    }
    let value = action["throttle"]["value"].as_i64()?;
    let unit = match action["throttle"]["unit"].as_str() {
        Some("HOURS") => 3_600_000,
        Some("DAYS") => 86_400_000,
        _ => 60_000,
    };
    Some(value.saturating_mul(unit))
}

/// The actions due on the ACTIVE alerts of `alerts`: each alert's
/// `action_execution_results` is stamped with `now` for the actions that
/// run. Returns the messages to send and whether any alert changed.
pub(super) fn due(
    alerts: &mut [Value],
    monitor: &Value,
    ctx: &Value,
    now: i64,
) -> (Vec<Dispatch>, bool) {
    let mut sends = Vec::new();
    let mut changed = false;
    for alert in alerts.iter_mut() {
        if alert["state"] != "ACTIVE" {
            continue;
        }
        let trigger = monitor["triggers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| t.get("query_level_trigger"))
            .find(|t| t["id"] == alert["trigger_id"]);
        let Some(trigger) = trigger else { continue };
        let mut local = ctx.clone();
        local["alert"] = alert.clone();
        local["trigger"] = trigger.clone();
        for action in trigger["actions"].as_array().into_iter().flatten() {
            let action_id = action["id"].as_str().unwrap_or_default();
            let results = alert["action_execution_results"].as_array_mut();
            let last = results.as_ref().and_then(|r| {
                r.iter()
                    .find(|e| e["action_id"] == action_id)
                    .and_then(|e| e["last_execution_time"].as_i64())
            });
            let held = throttle_ms(action)
                .is_some_and(|window| last.is_some_and(|at| now.saturating_sub(at) < window));
            if held {
                continue;
            }
            let source = action["message_template"]["source"]
                .as_str()
                .unwrap_or_default();
            sends.push(Dispatch {
                alert_id: alert["id"].as_str().unwrap_or_default().to_owned(),
                action_id: action_id.to_owned(),
                action_name: action["name"].as_str().unwrap_or_default().to_owned(),
                destination: action["destination_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                text: render(source, &local),
            });
            stamp(alert, action_id, Some(now));
            changed = true;
        }
    }
    (sends, changed)
}

/// Sets, or clears with `None`, the last run of `action_id` on `alert`.
pub(super) fn stamp(alert: &mut Value, action_id: &str, at: Option<i64>) {
    let Some(list) = alert["action_execution_results"].as_array_mut() else {
        return;
    };
    list.retain(|e| e["action_id"] != action_id);
    if let Some(at) = at {
        list.push(json!({"action_id": action_id, "last_execution_time": at, "throttled_count": 0}));
    }
}

/// Moves the alert `id` to ERROR for a failed action and lets it run again.
pub(super) fn failed(alerts: &mut [Value], send: &Dispatch, why: &str, now: i64) -> bool {
    let Some(alert) = alerts
        .iter_mut()
        .find(|a| a["id"] == send.alert_id.as_str())
    else {
        return false;
    };
    stamp(alert, &send.action_id, None);
    alert["state"] = json!("ERROR");
    alert["error_message"] = json!(format!("Failed running action {}: {why}", send.action_name));
    alert["version"] = json!(alert["version"].as_i64().unwrap_or(0).saturating_add(1));
    alert["last_notification_time"] = json!(now);
    true
}
