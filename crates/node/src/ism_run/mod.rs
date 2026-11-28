//! The index state management runner's decision: given a managed index's
//! record, its policy and the time, what the record becomes and when it next
//! needs evaluating. Pure: it reads nothing and writes nothing, so the
//! schedule that calls it (one derived time per index, no polling) and the
//! actions it will hand work to are separate concerns.

use serde_json::{Value, json};

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

/// Evaluates one managed index at `now` (epoch milliseconds).
#[must_use]
pub fn evaluate(record: &Value, policy: &Value, now: i64) -> Step {
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
    let has_actions = state["actions"].as_array().is_some_and(|a| !a.is_empty());
    if has_actions {
        return Step {
            record: out,
            next_eval: None,
        };
    }
    let mut earliest: Option<i64> = None;
    for t in state["transitions"].as_array().into_iter().flatten() {
        match due(&t["conditions"], record) {
            Some(at) if at <= now => {
                let target = t["state_name"].as_str().unwrap_or_default();
                out["state"] = json!({"name": target, "start_time": now});
                out["action"] = Value::Null;
                out["step"] = Value::Null;
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
mod tests {
    use super::*;
    use serde_json::json;

    const DAY: i64 = 86_400_000;

    fn policy(condition: Value) -> Value {
        json!({"policy_id":"p","default_state":"hot","states":[
            {"name":"hot","actions":[],"transitions":[{"state_name":"ro","conditions":condition}]},
            {"name":"ro","actions":[{"read_only":{}}],"transitions":[]}]})
    }

    fn record() -> Value {
        json!({"policy_id":"p","index":"logs","index_creation_date":1000,
               "state":{"name":"hot","start_time":1000},"enabled":true})
    }

    #[test]
    fn the_first_run_initialises_and_asks_to_run_again_now() {
        let s = evaluate(&record(), &policy(json!({"min_index_age":"1d"})), 5000);
        assert_eq!(
            s.record["info"]["message"],
            "Successfully initialized policy: p"
        );
        assert_eq!(
            s.record["retry_info"],
            json!({"failed":false,"consumed_retries":0})
        );
        assert_eq!(s.next_eval, Some(5000));
    }

    #[test]
    fn an_unmet_age_waits_until_the_index_is_old_enough() {
        let first = evaluate(&record(), &policy(json!({"min_index_age":"1d"})), 5000).record;
        let s = evaluate(&first, &policy(json!({"min_index_age":"1d"})), 6000);
        assert_eq!(s.record["step"]["step_status"], "condition_not_met");
        assert_eq!(s.record["step"]["name"], "attempt_transition_step");
        assert_eq!(s.record["action"]["name"], "transition");
        assert_eq!(
            s.record["info"]["message"],
            "Evaluating transition conditions [index=logs]"
        );
        assert_eq!(s.next_eval, Some(1000 + DAY));
    }

    #[test]
    fn a_met_age_moves_to_the_target_state() {
        let p = policy(json!({"min_index_age":"1d"}));
        let first = evaluate(&record(), &p, 5000).record;
        let s = evaluate(&first, &p, 1000 + DAY);
        assert_eq!(
            s.record["state"],
            json!({"name":"ro","start_time":1000 + DAY})
        );
        assert!(s.record["action"].is_null() && s.record["step"].is_null());
        assert_eq!(s.next_eval, Some(1000 + DAY));
    }

    #[test]
    fn a_transition_without_conditions_is_always_met() {
        let p = policy(Value::Null);
        let first = evaluate(&record(), &p, 5000).record;
        assert_eq!(evaluate(&first, &p, 6000).record["state"]["name"], "ro");
    }

    #[test]
    fn a_rollover_age_without_a_rollover_waits_for_an_outside_change() {
        let p = policy(json!({"min_rollover_age":"1h"}));
        let first = evaluate(&record(), &p, 5000).record;
        assert_eq!(evaluate(&first, &p, 9000).next_eval, None);
        let mut rolled = first;
        rolled["rolled_over_at"] = json!(7000);
        assert_eq!(
            evaluate(&rolled, &p, 9000).next_eval,
            Some(7000 + 3_600_000)
        );
    }

    #[test]
    fn a_state_with_actions_is_left_for_the_action_runner() {
        let p = policy(json!({"min_index_age":"1d"}));
        let mut r = evaluate(&record(), &p, 5000).record;
        r["state"] = json!({"name":"ro","start_time":5000});
        let s = evaluate(&r, &p, 6000);
        assert_eq!(s.record, r);
        assert_eq!(s.next_eval, None);
    }
}
