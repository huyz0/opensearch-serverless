//! A policy as a client puts it: checked against what a serverless node can
//! run, so that an action or condition it cannot honour is refused at put
//! and never at run.

use serde_json::{Map, Value, json};

/// Bounds on a policy's shape.
const MAX_STATES: usize = 50;
const MAX_ACTIONS: usize = 20;

/// Actions a node runs.
const ACTIONS: &[&str] = &[
    "rollover",
    "delete",
    "read_only",
    "read_write",
    "snapshot",
    "alias",
    "replica_count",
];
/// Keys every action may carry beside its own.
const ACTION_COMMON: &[&str] = &["timeout", "retry"];
/// Rollover keys that are served.
const ROLLOVER_KEYS: &[&str] = &["min_index_age", "copy_alias"];
/// Transition conditions that are served.
const CONDITIONS: &[&str] = &["min_index_age", "min_rollover_age", "cron"];

/// Why an action is refused, for the ones the plugin has and this node does not.
fn action_refusal(name: &str) -> String {
    let why = match name {
        "force_merge" => "no native merge request exists",
        "index_priority" => "the setting has no effect on this node",
        "close" | "open" => "an index is never closed or opened here",
        "shrink" | "allocation" => "an index has no replicas or node placement to change here",
        "rollup" | "transform" => "rollup and transform jobs are not ported",
        "notification" => "notification channels are not served yet",
        _ => "it is not an action of the plugin",
    };
    format!("action [{name}] is not supported: {why}")
}

fn condition_refusal(name: &str) -> String {
    let why = match name {
        "min_doc_count" | "min_size" | "min_primary_shard_size" => {
            "a count or a size would need a request per index over time"
        }
        _ => "it is not a condition of the plugin",
    };
    format!("condition [{name}] is not supported: {why}")
}

fn object<'a>(v: &'a Value, what: &str) -> Result<&'a Map<String, Value>, String> {
    v.as_object()
        .ok_or_else(|| format!("[{what}] must be an object"))
}

fn string<'a>(m: &'a Map<String, Value>, key: &str, what: &str) -> Result<&'a str, String> {
    match m.get(key).and_then(Value::as_str) {
        Some(s) if !s.is_empty() => Ok(s),
        _ => Err(format!("{what} needs a non-empty [{key}]")),
    }
}

fn action(a: &Value, state: &str) -> Result<(), String> {
    let m = object(a, "action")?;
    let mut own = m.keys().filter(|k| !ACTION_COMMON.contains(&k.as_str()));
    let (Some(name), None) = (own.next(), own.next()) else {
        return Err(format!(
            "an action of state [{state}] names exactly one action"
        ));
    };
    if !ACTIONS.contains(&name.as_str()) {
        return Err(action_refusal(name));
    }
    let body = object(&m[name], name)?;
    match name.as_str() {
        "rollover" => {
            for key in body.keys() {
                if !ROLLOVER_KEYS.contains(&key.as_str()) {
                    return Err(match key.as_str() {
                        "min_doc_count" | "min_size" | "min_primary_shard_size" => {
                            condition_refusal(key)
                        }
                        _ => format!("rollover has no [{key}]"),
                    });
                }
            }
            if !body.contains_key("min_index_age") {
                return Err("rollover needs a [min_index_age] condition".to_owned());
            }
        }
        "snapshot" => {
            string(body, "repository", "snapshot")?;
            string(body, "snapshot", "snapshot")?;
        }
        "replica_count" => {
            if body.get("number_of_replicas").and_then(Value::as_i64) != Some(0) {
                return Err("replica_count accepts only [number_of_replicas] 0 here".to_owned());
            }
        }
        "alias" if !body.get("actions").is_some_and(Value::is_array) => {
            return Err("alias needs an [actions] array".to_owned());
        }
        _ => {}
    }
    Ok(())
}

fn transition(t: &Value, names: &[&str]) -> Result<(), String> {
    let m = object(t, "transition")?;
    let target = string(m, "state_name", "a transition")?;
    if !names.contains(&target) {
        return Err(format!(
            "transition names state [{target}], which does not exist"
        ));
    }
    if let Some(conditions) = m.get("conditions") {
        let c = object(conditions, "conditions")?;
        if c.len() > 1 {
            return Err("a transition takes at most one condition".to_owned());
        }
        for key in c.keys() {
            if !CONDITIONS.contains(&key.as_str()) {
                return Err(condition_refusal(key));
            }
        }
    }
    Ok(())
}

/// Checks the `policy` object of a put and returns it as stored: the plugin's
/// fields, with `policy_id`, `schema_version` and `last_updated_time` set.
///
/// # Errors
///
/// The reason the policy is refused, for a 400.
pub(super) fn checked(id: &str, body: &Value, now_millis: u64) -> Result<Value, String> {
    let policy = object(body.get("policy").unwrap_or(&Value::Null), "policy")?;
    let default_state = string(policy, "default_state", "a policy")?;
    let states = policy
        .get("states")
        .and_then(Value::as_array)
        .filter(|s| !s.is_empty())
        .ok_or("a policy needs a non-empty [states] array")?;
    if states.len() > MAX_STATES {
        return Err(format!("a policy holds at most {MAX_STATES} states"));
    }
    let mut names = Vec::with_capacity(states.len());
    for state in states {
        let name = string(object(state, "state")?, "name", "a state")?;
        if names.contains(&name) {
            return Err(format!("state [{name}] is named twice"));
        }
        names.push(name);
    }
    if !names.contains(&default_state) {
        return Err(format!(
            "default_state [{default_state}] is not a state of the policy"
        ));
    }
    for state in states {
        let m = object(state, "state")?;
        let name = string(m, "name", "a state")?;
        let actions = m
            .get("actions")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        if actions.len() > MAX_ACTIONS {
            return Err(format!(
                "state [{name}] holds at most {MAX_ACTIONS} actions"
            ));
        }
        for a in actions {
            action(a, name)?;
        }
        for t in m
            .get("transitions")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice)
        {
            transition(t, &names)?;
        }
    }
    let mut stored = policy.clone();
    stored.insert("policy_id".to_owned(), json!(id));
    stored.insert("schema_version".to_owned(), json!(1));
    stored.insert("last_updated_time".to_owned(), json!(now_millis));
    Ok(Value::Object(stored))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(states: &str) -> Value {
        serde_json::from_str(&format!(
            r#"{{"policy":{{"default_state":"a","states":{states}}}}}"#
        ))
        .unwrap()
    }

    #[test]
    fn a_minimal_policy_is_stored_with_its_id_and_time() {
        let stored = checked(
            "p",
            &put(r#"[{"name":"a","actions":[],"transitions":[]}]"#),
            9,
        )
        .unwrap();
        assert_eq!(stored["policy_id"], "p");
        assert_eq!(stored["last_updated_time"], 9);
        assert_eq!(stored["schema_version"], 1);
    }

    #[test]
    fn what_the_node_cannot_run_is_refused_by_name() {
        let cases = [
            (
                r#"[{"name":"a","actions":[{"force_merge":{}}]}]"#,
                "force_merge",
            ),
            (
                r#"[{"name":"a","actions":[{"rollover":{"min_doc_count":1}}]}]"#,
                "min_doc_count",
            ),
            (
                r#"[{"name":"a","actions":[{"rollover":{}}]}]"#,
                "min_index_age",
            ),
            (
                r#"[{"name":"a","actions":[{"replica_count":{"number_of_replicas":2}}]}]"#,
                "replica_count",
            ),
            (
                r#"[{"name":"a","actions":[{"delete":{},"read_only":{}}]}]"#,
                "exactly one",
            ),
            (
                r#"[{"name":"a","transitions":[{"state_name":"a","conditions":{"min_size":"1gb"}}]}]"#,
                "min_size",
            ),
            (
                r#"[{"name":"a","transitions":[{"state_name":"a","conditions":{"min_index_age":"1d","cron":{}}}]}]"#,
                "at most one",
            ),
        ];
        for (states, word) in cases {
            let e = checked("p", &put(states), 0).unwrap_err();
            assert!(e.contains(word), "{word}: {e}");
        }
    }

    #[test]
    fn the_bounds_on_states_and_actions_hold() {
        let many: Vec<String> = (0..51).map(|i| format!(r#"{{"name":"s{i}"}}"#)).collect();
        let body = serde_json::from_str(&format!(
            r#"{{"policy":{{"default_state":"s0","states":[{}]}}}}"#,
            many.join(",")
        ))
        .unwrap();
        assert!(checked("p", &body, 0).unwrap_err().contains("50 states"));
        let acts = vec![r#"{"delete":{}}"#; 21].join(",");
        let e = checked(
            "p",
            &put(&format!(r#"[{{"name":"a","actions":[{acts}]}}]"#)),
            0,
        )
        .unwrap_err();
        assert!(e.contains("20 actions"), "{e}");
    }
}
