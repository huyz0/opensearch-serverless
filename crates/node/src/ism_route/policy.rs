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
    "notification",
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
        _ => return format!("Invalid field: [{name}] found in Actions."),
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

/// The time units a setting value may end in.
const UNITS: &[&str] = &["nanos", "micros", "ms", "s", "m", "h", "d"];

/// Checks a `min_index_age` style value the way the plugin's time setting does.
fn time_value(setting: &str, v: &Value) -> Result<(), String> {
    let text = v.as_str().unwrap_or_default();
    let digits = text.chars().take_while(char::is_ascii_digit).count();
    let (number, unit) = text.split_at(digits);
    if number.is_empty() || !UNITS.contains(&unit) {
        return Err(format!(
            "failed to parse setting [{setting}] with value [{text}] as a time value: unit is missing or unrecognized"
        ));
    }
    Ok(())
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
        "notification" => {
            if body.contains_key("destination") {
                return Err(
                    "notification needs a [channel]: destinations are not served".to_owned(),
                );
            }
            string(
                object(body.get("channel").unwrap_or(&Value::Null), "channel")?,
                "id",
                "notification channel",
            )?;
            let template = object(
                body.get("message_template").unwrap_or(&Value::Null),
                "message_template",
            )?;
            string(template, "source", "message_template")?;
        }
        "alias" if !body.get("actions").is_some_and(Value::is_array) => {
            return Err("alias needs an [actions] array".to_owned());
        }
        _ => {}
    }
    Ok(())
}

fn transition(t: &Value, state: &str, names: &[&str]) -> Result<(), String> {
    let m = object(t, "transition")?;
    let target = string(m, "state_name", "a transition")?;
    if !names.contains(&target) {
        return Err(format!(
            "Policy contains a transition in state={state} pointing to a nonexistent state={target}"
        ));
    }
    if let Some(conditions) = m.get("conditions") {
        let c = object(conditions, "conditions")?;
        if c.len() > 1 {
            return Err("a transition takes at most one condition".to_owned());
        }
        for (key, value) in c {
            if !CONDITIONS.contains(&key.as_str()) {
                return Err(condition_refusal(key));
            }
            if key != "cron" {
                time_value(key, value)?;
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
    let default_state = match policy.get("default_state").and_then(Value::as_str) {
        Some(s) if !s.is_empty() => s,
        _ => return Err("default_state is null".to_owned()),
    };
    let states = policy
        .get("states")
        .and_then(Value::as_array)
        .filter(|s| !s.is_empty())
        .ok_or("Policy must contain at least one State")?;
    if states.len() > MAX_STATES {
        return Err(format!("a policy holds at most {MAX_STATES} states"));
    }
    let mut names = Vec::with_capacity(states.len());
    for state in states {
        let name = string(object(state, "state")?, "name", "a state")?;
        if names.contains(&name) {
            return Err("Policy cannot have duplicate state names".to_owned());
        }
        names.push(name);
    }
    if !names.contains(&default_state) {
        return Err("Policy must have a valid default state".to_owned());
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
            transition(t, name, &names)?;
        }
    }
    let mut stored = policy.clone();
    normalise(&mut stored, now_millis);
    stored.insert("policy_id".to_owned(), json!(id));
    stored.insert("schema_version".to_owned(), json!(1));
    stored.insert("last_updated_time".to_owned(), json!(now_millis));
    Ok(Value::Object(stored))
}

/// What the plugin fills in when a policy is stored: `error_notification`
/// and `ism_template` as null, each template's time, and every action's default retry.
fn normalise(policy: &mut Map<String, Value>, now_millis: u64) {
    policy.entry("error_notification").or_insert(Value::Null);
    policy.entry("ism_template").or_insert(Value::Null);
    let template = policy.get_mut("ism_template");
    let templates = match template {
        Some(Value::Array(a)) => a.iter_mut().collect(),
        Some(t @ Value::Object(_)) => vec![t],
        _ => Vec::new(),
    };
    for t in templates {
        if let Some(m) = t.as_object_mut() {
            m.entry("last_updated_time").or_insert(json!(now_millis));
        }
    }
    let states = policy.get_mut("states").and_then(Value::as_array_mut);
    for state in states.into_iter().flatten() {
        let actions = state.get_mut("actions").and_then(Value::as_array_mut);
        for action in actions.into_iter().flatten() {
            if let Some(m) = action.as_object_mut() {
                m.entry("retry").or_insert_with(
                    || json!({"count": 3, "backoff": "exponential", "delay": "1m"}),
                );
            }
        }
        if let Some(m) = state.as_object_mut() {
            m.entry("actions").or_insert_with(|| json!([]));
            m.entry("transitions").or_insert_with(|| json!([]));
        }
    }
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
        assert!(stored["error_notification"].is_null());
        assert!(stored["ism_template"].is_null());
    }

    #[test]
    fn every_action_gets_the_default_retry() {
        let stored = checked(
            "p",
            &put(
                r#"[{"name":"a","actions":[{"delete":{}},{"read_only":{},"retry":{"count":1}}]}]"#,
            ),
            0,
        )
        .unwrap();
        let a = &stored["states"][0]["actions"];
        assert_eq!(a[0]["retry"]["count"], 3);
        assert_eq!(a[1]["retry"]["count"], 1);
    }

    #[test]
    fn a_notification_names_a_channel_and_a_destination_is_refused() {
        let ok = put(
            r#"[{"name":"a","actions":[{"notification":{"channel":{"id":"c1"},"message_template":{"source":"hi"}}}]}]"#,
        );
        assert!(checked("p", &ok, 0).is_ok());
        for (states, word) in [
            (
                r#"[{"name":"a","actions":[{"notification":{"destination":{"chime":{"url":"u"}},"message_template":{"source":"hi"}}}]}]"#,
                "channel",
            ),
            (
                r#"[{"name":"a","actions":[{"notification":{"channel":{"id":"c1"},"message_template":{}}}]}]"#,
                "source",
            ),
            (
                r#"[{"name":"a","actions":[{"notification":{}}]}]"#,
                "channel",
            ),
        ] {
            let err = checked("p", &put(states), 0).unwrap_err();
            assert!(err.contains(word), "{err}");
        }
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
                r#"[{"name":"a","transitions":[{"state_name":"a","conditions":{"min_index_age":"soon"}}]}]"#,
                "unit is missing",
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
