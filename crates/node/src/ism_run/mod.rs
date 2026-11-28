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
    out["action"] = json!({"name": name, "start_time": now, "index": at, "failed": failed,
        "consumed_retries": 0, "last_retry_time": null});
    out["step"] = json!({"name": step, "start_time": now, "step_status": status});
    out["info"] = json!({"message": message});
    out["retry_info"] = json!({"failed": failed, "consumed_retries": 0});
    if failed {
        return Step {
            record: out,
            next_eval: None,
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
mod tests {
    use super::*;
    use backend::Unsupported;
    use serde_json::json;

    fn ev(record: &Value, policy: &Value, now: i64) -> Step {
        evaluate(record, policy, now, &Unsupported)
    }

    /// A backend that records the blocks it is asked for.
    #[derive(Default)]
    struct Fake {
        asked: std::sync::Mutex<Vec<(String, bool)>>,
        refuse: bool,
    }

    impl Backend for Fake {
        fn set_replicas(&self, index: &str, n: u32) -> Result<(), String> {
            if self.refuse {
                return Err("index [logs] is missing".to_owned());
            }
            self.asked
                .lock()
                .unwrap()
                .push((format!("{index}:{n}"), false));
            Ok(())
        }

        fn delete(&self, index: &str) -> Result<(), String> {
            if self.refuse {
                return Err("index [logs] is missing".to_owned());
            }
            self.asked.lock().unwrap().push((index.to_owned(), false));
            Ok(())
        }

        fn set_alias(
            &self,
            index: &str,
            add: bool,
            alias: &str,
            write: Option<bool>,
        ) -> Result<(), String> {
            if self.refuse {
                return Err("index [logs] is missing".to_owned());
            }
            let verb = if add { "add" } else { "remove" };
            self.asked
                .lock()
                .unwrap()
                .push((format!("{verb} {alias} {index} {write:?}"), false));
            Ok(())
        }

        fn set_read_only(&self, index: &str, on: bool) -> Result<(), String> {
            if self.refuse {
                return Err("index [logs] is missing".to_owned());
            }
            self.asked.lock().unwrap().push((index.to_owned(), on));
            Ok(())
        }
    }

    fn acting(actions: Value) -> Value {
        json!({"policy_id":"p","default_state":"a","states":[
            {"name":"a","actions":actions,"transitions":[{"state_name":"b"}]},
            {"name":"b","actions":[],"transitions":[]}]})
    }

    fn started() -> Value {
        let mut r = record();
        r["state"] = json!({"name":"a","start_time":1000});
        r["info"] = json!({"message":"Successfully initialized policy: p"});
        r
    }

    #[test]
    fn a_read_only_action_blocks_the_index_and_leaves_the_stock_shapes() {
        let p = acting(json!([{"read_only":{}}]));
        let fake = Fake::default();
        let s = evaluate(&started(), &p, 7000, &fake);
        assert_eq!(*fake.asked.lock().unwrap(), [("logs".to_owned(), true)]);
        assert_eq!(s.record["action"]["name"], "read_only");
        assert_eq!(s.record["action"]["index"], 0);
        assert_eq!(s.record["step"]["name"], "set_read_only");
        assert_eq!(s.record["step"]["step_status"], "completed");
        assert_eq!(
            s.record["info"]["message"],
            "Successfully set index to read-only [index=logs]"
        );
        assert_eq!(s.next_eval, Some(7000));
    }

    #[test]
    fn actions_run_in_order_then_the_transition_is_evaluated() {
        let p = acting(json!([{"read_only":{}},{"read_write":{}}]));
        let fake = Fake::default();
        let one = evaluate(&started(), &p, 7000, &fake).record;
        let two = evaluate(&one, &p, 7001, &fake).record;
        assert_eq!(two["step"]["name"], "set_read_write");
        assert_eq!(
            *fake.asked.lock().unwrap(),
            [("logs".to_owned(), true), ("logs".to_owned(), false)]
        );
        let moved = evaluate(&two, &p, 7002, &fake).record;
        assert_eq!(moved["state"]["name"], "b");
        assert!(moved["action_index"].is_null());
        assert_eq!(fake.asked.lock().unwrap().len(), 2, "nothing ran twice");
    }

    #[test]
    fn a_refused_block_fails_the_step_and_waits_for_a_retry() {
        let p = acting(json!([{"read_only":{}}]));
        let fake = Fake {
            refuse: true,
            ..Fake::default()
        };
        let s = evaluate(&started(), &p, 7000, &fake);
        assert_eq!(s.record["step"]["step_status"], "failed");
        assert_eq!(s.record["action"]["failed"], true);
        assert_eq!(s.record["retry_info"]["failed"], true);
        assert_eq!(s.record["info"]["message"], "index [logs] is missing");
        assert_eq!(s.next_eval, None);
        assert!(
            s.record["action_index"].is_null(),
            "the action is still the one to run"
        );
    }

    #[test]
    fn a_delete_action_deletes_the_index_and_marks_the_record_for_removal() {
        let p = acting(json!([{"delete":{}}]));
        let fake = Fake::default();
        let s = evaluate(&started(), &p, 7000, &fake);
        assert_eq!(*fake.asked.lock().unwrap(), [("logs".to_owned(), false)]);
        assert_eq!(s.record["step"]["name"], "attempt_delete");
        assert_eq!(s.record["step"]["step_status"], "completed");
        assert_eq!(s.record["deleted"], true);
        let refused = Fake {
            refuse: true,
            ..Fake::default()
        };
        let s = evaluate(&started(), &p, 7000, &refused);
        assert_eq!(s.record["step"]["step_status"], "failed");
        assert!(s.record["deleted"].is_null());
    }

    #[test]
    fn a_replica_count_action_sets_the_number_and_a_missing_number_fails() {
        let p = acting(json!([{"replica_count":{"number_of_replicas":0}}]));
        let fake = Fake::default();
        let s = evaluate(&started(), &p, 7000, &fake);
        assert_eq!(*fake.asked.lock().unwrap(), [("logs:0".to_owned(), false)]);
        assert_eq!(s.record["step"]["name"], "attempt_set_replica_count");
        assert_eq!(
            s.record["info"]["message"],
            "Successfully updated number_of_replicas to 0 [index=logs]"
        );
        let p = acting(json!([{"replica_count":{}}]));
        let s = evaluate(&started(), &p, 7000, &fake);
        assert_eq!(s.record["step"]["step_status"], "failed");
        assert_eq!(fake.asked.lock().unwrap().len(), 1, "nothing was asked");
    }

    #[test]
    fn an_alias_action_adds_and_removes_each_alias_in_order() {
        let p = acting(json!([{"alias":{"actions":[
            {"add":{"aliases":["a","b"],"is_write_index":true}},
            {"remove":{"alias":"old"}}]}}]));
        let fake = Fake::default();
        let s = evaluate(&started(), &p, 7000, &fake);
        assert_eq!(
            *fake.asked.lock().unwrap(),
            [
                ("add a logs Some(true)".to_owned(), false),
                ("add b logs Some(true)".to_owned(), false),
                ("remove old logs None".to_owned(), false),
            ]
        );
        assert_eq!(s.record["step"]["name"], "attempt_alias");
        assert_eq!(s.record["step"]["step_status"], "completed");
        assert_eq!(
            s.record["info"]["message"],
            "Successfully updated aliases [index=logs]"
        );
        let none = acting(json!([{"alias":{"actions":[]}}]));
        let s = evaluate(&started(), &none, 7000, &fake);
        assert_eq!(s.record["step"]["step_status"], "failed");
        let refused = Fake {
            refuse: true,
            ..Fake::default()
        };
        let s = evaluate(&started(), &p, 7000, &refused);
        assert_eq!(s.record["step"]["step_status"], "failed");
    }

    #[test]
    fn an_action_the_node_cannot_run_yet_fails_naming_itself() {
        let p = acting(json!([{"snapshot":{}}]));
        let s = ev(&started(), &p, 7000);
        assert_eq!(s.record["step"]["step_status"], "failed");
        assert_eq!(
            s.record["info"]["message"],
            "action [snapshot] cannot run on this node yet"
        );
    }

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
        let s = ev(&record(), &policy(json!({"min_index_age":"1d"})), 5000);
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
        let first = ev(&record(), &policy(json!({"min_index_age":"1d"})), 5000).record;
        let s = ev(&first, &policy(json!({"min_index_age":"1d"})), 6000);
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
        let first = ev(&record(), &p, 5000).record;
        let s = ev(&first, &p, 1000 + DAY);
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
        let first = ev(&record(), &p, 5000).record;
        assert_eq!(ev(&first, &p, 6000).record["state"]["name"], "ro");
    }

    #[test]
    fn a_rollover_age_without_a_rollover_waits_for_an_outside_change() {
        let p = policy(json!({"min_rollover_age":"1h"}));
        let first = ev(&record(), &p, 5000).record;
        assert_eq!(ev(&first, &p, 9000).next_eval, None);
        let mut rolled = first;
        rolled["rolled_over_at"] = json!(7000);
        assert_eq!(ev(&rolled, &p, 9000).next_eval, Some(7000 + 3_600_000));
    }
}

#[cfg(test)]
mod schedule_tests;
