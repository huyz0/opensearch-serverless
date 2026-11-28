//! The index state management runner's decision: given a managed index's
//! record, its policy and the time, what the record becomes and when it next
//! needs evaluating. Pure: it reads nothing and writes nothing, so the
//! schedule that calls it (one derived time per index, no polling) and the
//! actions it will hand work to are separate concerns.

use backend::Backend;
use serde_json::{Value, json};

mod actions;
pub mod backend;
#[cfg(feature = "coordinator")]
pub mod node_backend;
pub mod schedule;

/// What one evaluation decided.
#[derive(Debug, PartialEq)]
pub struct Step {
    /// The record after the evaluation; equal to the input when nothing changed.
    pub record: Value,
    /// When the index next needs evaluating, in epoch milliseconds: `None`
    /// when only an outside change (a policy change, a retry) can move it.
    pub next_eval: Option<i64>,
}

/// A policy time value (`30s`, `1d`) in milliseconds; `None` if malformed.
fn millis(value: &Value) -> Option<i64> {
    let text = value.as_str()?;
    let digits = text.chars().take_while(char::is_ascii_digit).count();
    let (number, unit) = text.split_at(digits);
    let n: i64 = number.parse().ok()?;
    let per = match unit {
        "ms" => 1,
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        "micros" | "nanos" => return Some(0),
        _ => return None,
    };
    Some(n.saturating_mul(per))
}

/// When a transition's conditions are met: `Some(t)` is the first moment,
/// `None` is "not by any time this record fixes" (a rollover that has not
/// happened; a cron expression, which the runner does not evaluate yet).
fn due(conditions: &Value, record: &Value) -> Option<i64> {
    if conditions.is_null() {
        return Some(i64::MIN);
    }
    if let Some(age) = conditions.get("min_index_age").and_then(millis) {
        return record["index_creation_date"]
            .as_i64()
            .map(|c| c.saturating_add(age));
    }
    let age = conditions.get("min_rollover_age").and_then(millis)?;
    record["rolled_over_at"]
        .as_i64()
        .map(|r| r.saturating_add(age))
}

fn state_of<'a>(policy: &'a Value, name: &str) -> Option<&'a Value> {
    policy["states"]
        .as_array()?
        .iter()
        .find(|s| s["name"] == name)
}

/// How long to wait before the retry after `consumed` retries: the action's
/// `retry.delay`, kept, grown by one delay a retry, or doubled a retry.
fn wait(action: &Value, consumed: u64) -> i64 {
    let delay = millis(&action["retry"]["delay"]).unwrap_or(60_000);
    let times = match action["retry"]["backoff"].as_str() {
        Some("constant") => 1,
        Some("linear") => i64::try_from(consumed)
            .unwrap_or(i64::MAX)
            .saturating_add(1),
        _ => 1_i64
            .checked_shl(u32::try_from(consumed).unwrap_or(u32::MAX))
            .unwrap_or(i64::MAX),
    };
    delay.saturating_mul(times)
}

/// Runs the action at position `at` of the state, once, and records how it went.
fn run_action(record: &Value, action: &Value, at: usize, now: i64, backend: &dyn Backend) -> Step {
    let mut out = record.clone();
    let index = record["index"].as_str().unwrap_or_default();
    let name = action
        .as_object()
        .and_then(|m| {
            m.keys()
                .find(|k| !matches!(k.as_str(), "timeout" | "retry"))
        })
        .map_or("", String::as_str);
    let (step, result) = actions::perform(name, action, index, backend);
    let failed = result.is_err();
    let (status, message) = match result {
        Ok(m) => ("completed", m),
        Err(m) => ("failed", m),
    };
    // A retry in progress: this action failed before and still has retries.
    let resumed = record["step"]["step_status"] == "failed"
        && record["action"]["failed"] == false
        && record["action"]["index"].as_u64() == u64::try_from(at).ok();
    let consumed = if resumed {
        record["action"]["consumed_retries"].as_u64().unwrap_or(0)
    } else {
        0
    };
    let allowed = action["retry"]["count"].as_u64().unwrap_or(0);
    let retry_at =
        (failed && consumed < allowed).then(|| now.saturating_add(wait(action, consumed)));
    let (spent, last) = match retry_at {
        Some(_) => (consumed.saturating_add(1), json!(now)),
        None if resumed => (consumed, record["action"]["last_retry_time"].clone()),
        None => (consumed, Value::Null),
    };
    let exhausted = failed && retry_at.is_none();
    out["action"] = json!({"name": name, "start_time": now, "index": at, "failed": exhausted,
        "consumed_retries": spent, "last_retry_time": last});
    out["step"] = json!({"name": step, "start_time": now, "step_status": status});
    out["info"] = json!({"message": message});
    out["retry_info"] = json!({"failed": exhausted, "consumed_retries": spent});
    if failed {
        return Step {
            record: out,
            next_eval: retry_at,
        };
    }
    out["action_index"] = json!(at.saturating_add(1));
    if name == "delete" {
        // The index is gone: the schedule drops this record.
        out["deleted"] = json!(true);
    }
    Step {
        record: out,
        next_eval: Some(now),
    }
}

/// Evaluates one managed index at `now` (epoch milliseconds).
#[must_use]
pub fn evaluate(record: &Value, policy: &Value, now: i64, backend: &dyn Backend) -> Step {
    let mut out = record.clone();
    let id = record["policy_id"].as_str().unwrap_or_default();
    if record["info"].is_null() {
        out["retry_info"] = json!({"failed": false, "consumed_retries": 0});
        out["info"] = json!({"message": format!("Successfully initialized policy: {id}")});
        return Step {
            record: out,
            next_eval: Some(now),
        };
    }
    let name = record["state"]["name"].as_str().unwrap_or_default();
    let Some(state) = state_of(policy, name) else {
        return Step {
            record: out,
            next_eval: None,
        };
    };
    let actions = state["actions"].as_array().map_or(&[][..], Vec::as_slice);
    let at = usize::try_from(record["action_index"].as_u64().unwrap_or(0)).unwrap_or(usize::MAX);
    if let Some(action) = actions.get(at) {
        return run_action(record, action, at, now, backend);
    }
    let mut earliest: Option<i64> = None;
    for t in state["transitions"].as_array().into_iter().flatten() {
        match due(&t["conditions"], record) {
            Some(at) if at <= now => {
                let target = t["state_name"].as_str().unwrap_or_default();
                out["state"] = json!({"name": target, "start_time": now});
                out["action"] = Value::Null;
                out["step"] = Value::Null;
                out["action_index"] = Value::Null;
                out["info"] = json!({"message": format!("Transitioning to {target} [index={}]", record["index"].as_str().unwrap_or_default())});
                return Step {
                    record: out,
                    next_eval: Some(now),
                };
            }
            Some(at) => earliest = Some(earliest.map_or(at, |e| e.min(at))),
            None => {}
        }
    }
    let started = record["action"]["start_time"].as_i64().unwrap_or(now);
    out["action"] = json!({"name": "transition", "start_time": started, "index": -1,
        "failed": false, "consumed_retries": 0, "last_retry_time": null});
    out["step"] = json!({"name": "attempt_transition_step", "start_time": started,
        "step_status": "condition_not_met"});
    out["info"] = json!({"message": format!("Evaluating transition conditions [index={}]", record["index"].as_str().unwrap_or_default())});
    Step {
        record: out,
        next_eval: earliest,
    }
}

#[cfg(test)]
mod schedule_tests;
#[cfg(test)]
mod tests;
