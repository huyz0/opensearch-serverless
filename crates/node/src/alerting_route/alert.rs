//! A monitor's alerts as the plugin's API shows them, and the moves a run and
//! an acknowledge make: a triggered trigger opens an ACTIVE alert (ERROR when
//! its condition failed), a trigger that clears completes it, an acknowledge
//! moves ACTIVE to ACKNOWLEDGED.

use oss_meta::new_id;
use serde_json::{Value, json};

/// The alert document's schema version, as the plugin writes it.
const SCHEMA_VERSION: i64 = 6;

/// One trigger's outcome in a run.
pub(super) struct Outcome<'a> {
    pub(super) id: &'a str,
    pub(super) name: &'a str,
    pub(super) severity: &'a str,
    pub(super) triggered: bool,
    pub(super) error: Option<&'a str>,
}

/// The monitor a run or an acknowledge is for.
pub(super) struct Subject<'a> {
    pub(super) id: &'a str,
    pub(super) name: &'a str,
    pub(super) version: i64,
}

/// The alerts of a stored set: its `alerts` array, none for an empty set.
pub(super) fn parse(set: &[u8]) -> Result<Vec<Value>, String> {
    if set.is_empty() {
        return Ok(Vec::new());
    }
    let mut doc: Value = serde_json::from_slice(set).map_err(|e| e.to_string())?;
    match doc["alerts"].take() {
        Value::Array(items) => Ok(items),
        _ => Err("an alert set holds an alerts array".to_owned()),
    }
}

/// The set's stored form.
pub(super) fn write(alerts: Vec<Value>) -> Vec<u8> {
    json!({"alerts": alerts}).to_string().into_bytes()
}

fn opened(subject: &Subject<'_>, t: &Outcome<'_>, now: i64, execution: &str) -> Value {
    json!({
        "id": new_id().unwrap_or_default(),
        "version": 1,
        "monitor_id": subject.id,
        "workflow_id": "",
        "workflow_name": "",
        "associated_alert_ids": [],
        "schema_version": SCHEMA_VERSION,
        "monitor_version": subject.version,
        "monitor_name": subject.name,
        "execution_id": execution,
        "trigger_id": t.id,
        "trigger_name": t.name,
        "finding_ids": [],
        "related_doc_ids": [],
        "state": if t.error.is_some() { "ERROR" } else { "ACTIVE" },
        "error_message": t.error,
        "alert_history": [],
        "severity": t.severity,
        "action_execution_results": [],
        "start_time": now,
        "last_notification_time": now,
        "end_time": null,
        "acknowledged_time": null,
    })
}

/// The set after a run, or `None` when the run changes nothing.
pub(super) fn after_run(
    current: &[Value],
    subject: &Subject<'_>,
    outcomes: &[Outcome<'_>],
    now: i64,
    execution: &str,
) -> Option<Vec<Value>> {
    let mut next = Vec::new();
    let mut changed = false;
    for t in outcomes {
        let existing = current.iter().find(|a| a["trigger_id"] == t.id);
        match (existing, t.triggered) {
            (None, true) => {
                next.push(opened(subject, t, now, execution));
                changed = true;
            }
            (Some(old), true) => {
                let (state, error) = (
                    old["state"].as_str().unwrap_or(""),
                    old["error_message"].as_str(),
                );
                let (want_state, want_error) = if t.error.is_some() {
                    ("ERROR", t.error)
                } else if state == "ERROR" {
                    ("ACTIVE", None)
                } else {
                    (state, None)
                };
                if want_state == state && want_error == error {
                    next.push(old.clone());
                } else {
                    let mut a = old.clone();
                    a["state"] = json!(want_state);
                    a["error_message"] = json!(want_error);
                    a["version"] = json!(old["version"].as_i64().unwrap_or(0).saturating_add(1));
                    a["last_notification_time"] = json!(now);
                    next.push(a);
                    changed = true;
                }
            }
            // A trigger that cleared completes its alert.
            (Some(_), false) => changed = true,
            (None, false) => {}
        }
    }
    // An alert of a trigger the monitor no longer has is gone with it.
    if current
        .iter()
        .any(|a| !outcomes.iter().any(|t| a["trigger_id"] == t.id))
    {
        changed = true;
    }
    changed.then_some(next)
}

/// What an acknowledge of `ids` did to `current`.
pub(super) struct Acknowledged {
    pub(super) alerts: Vec<Value>,
    pub(super) success: Vec<String>,
    pub(super) failed: Vec<(String, String)>,
    pub(super) changed: bool,
}

pub(super) fn acknowledge(current: &[Value], ids: &[String], now: i64) -> Acknowledged {
    let mut out = Acknowledged {
        alerts: current.to_vec(),
        success: Vec::new(),
        failed: Vec::new(),
        changed: false,
    };
    for id in ids {
        match out.alerts.iter_mut().find(|a| a["id"] == id.as_str()) {
            None => out.failed.push((
                id.clone(),
                // ALLOC: error path; one message.
                format!("Alert: {id} does not exist (it may have already completed)."),
            )),
            Some(a) => match a["state"].as_str() {
                Some("ACKNOWLEDGED") => out.failed.push((
                    id.clone(),
                    "Alert has already been acknowledged.".to_owned(),
                )),
                Some("ACTIVE") => {
                    a["state"] = json!("ACKNOWLEDGED");
                    a["acknowledged_time"] = json!(now);
                    a["version"] = json!(a["version"].as_i64().unwrap_or(0).saturating_add(1));
                    out.success.push(id.clone());
                    out.changed = true;
                }
                _ => out
                    .failed
                    .push((id.clone(), "Alert is not in the ACTIVE state.".to_owned())),
            },
        }
    }
    out
}
