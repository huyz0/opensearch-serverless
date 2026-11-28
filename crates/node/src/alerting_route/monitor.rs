//! A monitor's body as the stock plugin takes it: what a put checks, and
//! what is stored and answered.

use serde_json::{Map, Value, json};

/// The most triggers a monitor has, and actions a trigger has.
const MAX_TRIGGERS: usize = 100;
const MAX_ACTIONS: usize = 100;
/// The longest monitor name.
const MAX_NAME_CHARS: usize = 256;
/// The monitor kinds served; the body's `monitor_type` names one.
const KINDS: &[&str] = &[
    "query_level_monitor",
    "bucket_level_monitor",
    "doc_level_monitor",
];

/// A put's body, checked.
pub(super) struct Checked {
    /// The body to store, with the plugin's bookkeeping fields set.
    pub(super) stored: Value,
    pub(super) enabled: bool,
    /// The schedule as the index carries it.
    pub(super) schedule: String,
}

/// The seconds in one unit of a period schedule.
fn unit_secs(unit: &str) -> Option<u64> {
    match unit {
        "MINUTES" => Some(60),
        "HOURS" => Some(3_600),
        "DAYS" => Some(86_400),
        _ => None,
    }
}

/// The schedule of `body` in the index's text form, refusing one below
/// `min_interval_secs`.
fn schedule_of(body: &Value, min_interval_secs: u64) -> Result<String, String> {
    let schedule = body
        .get("schedule")
        .and_then(Value::as_object)
        .ok_or("a monitor needs its [schedule]")?;
    if let Some(period) = schedule.get("period").and_then(Value::as_object) {
        let interval = period
            .get("interval")
            .and_then(Value::as_u64)
            .filter(|n| *n > 0)
            .ok_or("a period schedule needs a positive [interval]")?;
        let unit = period
            .get("unit")
            .and_then(Value::as_str)
            .ok_or("a period schedule needs its [unit]")?;
        let secs = unit_secs(unit)
            .ok_or_else(|| format!("[unit] must be MINUTES, HOURS or DAYS, not [{unit}]"))?;
        if interval.saturating_mul(secs) < min_interval_secs {
            return Err(format!(
                "a schedule must run at most every {min_interval_secs} seconds"
            ));
        }
        // ALLOC: one schedule text, at most the index's bound.
        return Ok(format!("period:{interval}:{unit}"));
    }
    if let Some(cron) = schedule.get("cron").and_then(Value::as_object) {
        let expression = cron
            .get("expression")
            .and_then(Value::as_str)
            .filter(|e| !e.trim().is_empty())
            .ok_or("a cron schedule needs its [expression]")?;
        let timezone = cron
            .get("timezone")
            .and_then(Value::as_str)
            .ok_or("a cron schedule needs its [timezone]")?;
        // ALLOC: one schedule text, at most the index's bound.
        return Ok(format!("cron:{timezone}:{expression}"));
    }
    Err("a [schedule] holds a [period] or a [cron]".to_owned())
}

/// Checks the triggers' counts: each trigger is an object of one kind of
/// trigger, which holds its actions.
fn check_triggers(body: &Value) -> Result<(), String> {
    let Some(triggers) = body.get("triggers") else {
        return Ok(());
    };
    let triggers = triggers.as_array().ok_or("[triggers] must be an array")?;
    if triggers.len() > MAX_TRIGGERS {
        return Err(format!("a monitor holds at most {MAX_TRIGGERS} triggers"));
    }
    for trigger in triggers {
        let kinds = trigger.as_object().ok_or("a trigger must be an object")?;
        for inner in kinds.values() {
            let actions = inner.get("actions").and_then(Value::as_array);
            if actions.is_some_and(|a| a.len() > MAX_ACTIONS) {
                return Err(format!("a trigger holds at most {MAX_ACTIONS} actions"));
            }
        }
    }
    Ok(())
}

impl Checked {
    /// Checks a put's body and sets `type`, `last_update_time` and
    /// `enabled_time` as the plugin does.
    pub(super) fn from_put(
        body: &Value,
        now_ms: i64,
        min_interval_secs: u64,
    ) -> Result<Self, String> {
        let fields: &Map<String, Value> = body.as_object().ok_or("the body must be an object")?;
        let name = fields
            .get("name")
            .and_then(Value::as_str)
            .filter(|n| !n.trim().is_empty())
            .ok_or("a monitor needs its [name]")?;
        if name.chars().count() > MAX_NAME_CHARS {
            return Err(format!(
                "a monitor name is at most {MAX_NAME_CHARS} characters"
            ));
        }
        let kind = fields
            .get("monitor_type")
            .map_or(Ok("query_level_monitor"), |v| {
                v.as_str().ok_or("[monitor_type] must be a string")
            })?;
        if !KINDS.contains(&kind) {
            return Err(format!("unknown [monitor_type] [{kind}]"));
        }
        let enabled = fields.get("enabled").map_or(Ok(true), |v| {
            v.as_bool().ok_or("[enabled] must be true or false")
        })?;
        let inputs = fields
            .get("inputs")
            .and_then(Value::as_array)
            .filter(|i| !i.is_empty())
            .ok_or("a monitor needs its [inputs]")?;
        if inputs.iter().any(|i| !i.is_object()) {
            return Err("an input must be an object".to_owned());
        }
        check_triggers(body)?;
        let schedule = schedule_of(body, min_interval_secs)?;
        // ALLOC: the body as a map, at most the body bound, once a put.
        let mut stored = fields.clone();
        stored.insert("type".to_owned(), json!("monitor"));
        stored.insert("monitor_type".to_owned(), json!(kind));
        stored.insert("enabled".to_owned(), json!(enabled));
        stored.insert("last_update_time".to_owned(), json!(now_ms));
        stored.insert(
            "enabled_time".to_owned(),
            if enabled { json!(now_ms) } else { Value::Null },
        );
        stored.entry("schema_version").or_insert_with(|| json!(0));
        stored.entry("triggers").or_insert_with(|| json!([]));
        Ok(Self {
            stored: Value::Object(stored),
            enabled,
            schedule,
        })
    }
}
