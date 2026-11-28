//! A monitor's body as the stock plugin takes it: what a put checks, what
//! it fills in, and what is stored and answered.

use oss_meta::new_id;
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

/// A refused body: the plugin's status and message, and the exception it
/// wraps as the cause (`None` when it has none).
pub(super) struct Refused {
    pub(super) status: u16,
    pub(super) reason: String,
    pub(super) cause: Option<String>,
}

fn illegal(reason: impl Into<String>) -> Refused {
    let reason = reason.into();
    Refused {
        status: 400,
        cause: Some(format!("java.lang.IllegalArgumentException: {reason}")),
        reason,
    }
}

/// A put's body, checked.
pub(super) struct Checked {
    /// The body to store, with the plugin's defaults and bookkeeping set.
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
fn schedule_of(body: &Value, min_interval_secs: u64) -> Result<String, Refused> {
    let schedule = body
        .get("schedule")
        .and_then(Value::as_object)
        .ok_or_else(|| illegal("a monitor needs its [schedule]"))?;
    if let Some(period) = schedule.get("period").and_then(Value::as_object) {
        let interval = period.get("interval").and_then(Value::as_i64).unwrap_or(0);
        if interval <= 0 {
            return Err(illegal("Interval is not allowed to be 0 or negative"));
        }
        let unit = period
            .get("unit")
            .and_then(Value::as_str)
            .ok_or_else(|| illegal("a period schedule needs its [unit]"))?;
        let secs = unit_secs(unit).ok_or_else(|| {
            illegal(format!(
                "No enum constant java.time.temporal.ChronoUnit.{unit}"
            ))
        })?;
        if interval.unsigned_abs().saturating_mul(secs) < min_interval_secs {
            return Err(illegal(format!(
                "a schedule must run at most every {min_interval_secs} seconds"
            )));
        }
        // ALLOC: one schedule text, at most the index's bound.
        return Ok(format!("period:{interval}:{unit}"));
    }
    if let Some(cron) = schedule.get("cron").and_then(Value::as_object) {
        let expression = cron
            .get("expression")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let parts = expression.split_whitespace().count();
        if parts != 5 {
            return Err(illegal(format!(
                "Cron expression contains {parts} parts but we expect one of [5]"
            )));
        }
        let timezone = cron
            .get("timezone")
            .and_then(Value::as_str)
            .ok_or_else(|| illegal("a cron schedule needs its [timezone]"))?;
        // ALLOC: one schedule text, at most the index's bound.
        return Ok(format!("cron:{timezone}:{expression}"));
    }
    Err(illegal("a [schedule] holds a [period] or a [cron]"))
}

/// Adds `boost: 1.0` to every `match_all` and `match_none` in `query`, as
/// the plugin does when it writes the query back.
fn normalize_query(query: &mut Value) {
    match query {
        Value::Object(map) => {
            for (key, value) in map.iter_mut() {
                if matches!(key.as_str(), "match_all" | "match_none")
                    && let Value::Object(inner) = value
                {
                    inner.entry("boost").or_insert_with(|| json!(1.0));
                } else {
                    normalize_query(value);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(normalize_query),
        _ => {}
    }
}

/// Fills in what the plugin fills in for a composite aggregation: its page
/// `size` and each `terms` source's `missing_bucket` and `order`.
fn normalize_composites(aggs: &mut Value) {
    let Some(aggs) = aggs.as_object_mut() else {
        return;
    };
    for agg in aggs.values_mut() {
        let Some(composite) = agg.get_mut("composite").and_then(Value::as_object_mut) else {
            continue;
        };
        composite.entry("size").or_insert_with(|| json!(10));
        let sources = composite
            .get_mut("sources")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten();
        for source in sources {
            let terms = source
                .as_object_mut()
                .into_iter()
                .flat_map(|s| s.values_mut())
                .filter_map(|named| named.get_mut("terms").and_then(Value::as_object_mut));
            for terms in terms {
                terms
                    .entry("missing_bucket")
                    .or_insert_with(|| json!(false));
                terms.entry("order").or_insert_with(|| json!("asc"));
            }
        }
    }
}

/// An id for a trigger, action or query that came without one.
fn fresh_id() -> Result<Value, Refused> {
    new_id().map(Value::String).map_err(|e| Refused {
        status: 500,
        reason: e.to_string(),
        cause: None,
    })
}

fn set_id(object: &mut Map<String, Value>) -> Result<(), Refused> {
    if !object.get("id").is_some_and(Value::is_string) {
        object.insert("id".to_owned(), fresh_id()?);
    }
    Ok(())
}

/// Checks the triggers' counts and fills in their ids, and a bucket
/// trigger's `gap_policy`.
fn normalize_triggers(body: &mut Map<String, Value>) -> Result<(), Refused> {
    let Some(triggers) = body.get_mut("triggers") else {
        return Ok(());
    };
    let triggers = triggers
        .as_array_mut()
        .ok_or_else(|| illegal("[triggers] must be an array"))?;
    if triggers.len() > MAX_TRIGGERS {
        return Err(illegal(format!(
            "a monitor holds at most {MAX_TRIGGERS} triggers"
        )));
    }
    for trigger in triggers {
        let kinds = trigger
            .as_object_mut()
            .ok_or_else(|| illegal("a trigger must be an object"))?;
        for (kind, inner) in kinds.iter_mut() {
            let Some(inner) = inner.as_object_mut() else {
                continue;
            };
            set_id(inner)?;
            if kind == "bucket_level_trigger"
                && let Some(Value::Object(condition)) = inner.get_mut("condition")
            {
                condition
                    .entry("gap_policy")
                    .or_insert_with(|| json!("skip"));
            }
            let Some(actions) = inner.get_mut("actions").and_then(Value::as_array_mut) else {
                continue;
            };
            if actions.len() > MAX_ACTIONS {
                return Err(illegal(format!(
                    "a trigger holds at most {MAX_ACTIONS} actions"
                )));
            }
            for action in actions {
                if let Some(action) = action.as_object_mut() {
                    set_id(action)?;
                }
            }
        }
    }
    Ok(())
}

/// Fills in the inputs: a search's query and composites, a document-level
/// input's queries.
fn normalize_inputs(inputs: &mut [Value]) -> Result<(), Refused> {
    for input in inputs {
        let Some(input) = input.as_object_mut() else {
            return Err(illegal("an input must be an object"));
        };
        if let Some(search) = input.get_mut("search").and_then(Value::as_object_mut)
            && let Some(query) = search.get_mut("query")
        {
            normalize_query(query);
            if let Some(aggs) = query.get_mut("aggregations") {
                normalize_composites(aggs);
            }
        }
        if let Some(doc) = input
            .get_mut("doc_level_input")
            .and_then(Value::as_object_mut)
        {
            doc.entry("fan_out_enabled").or_insert_with(|| json!(true));
            let queries = doc
                .get_mut("queries")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten();
            for query in queries.filter_map(Value::as_object_mut) {
                set_id(query)?;
                query.entry("fields").or_insert_with(|| json!([]));
                query.entry("tags").or_insert_with(|| json!([]));
                query
                    .entry("query_field_names")
                    .or_insert_with(|| json!([]));
            }
        }
    }
    Ok(())
}

/// The index names of the plugin's own indices, shown in every monitor.
fn data_sources() -> Value {
    json!({
        "query_index": ".opensearch-alerting-queries",
        "findings_index": ".opensearch-alerting-finding-history-write",
        "findings_index_pattern": "<.opensearch-alerting-finding-history-{now/d}-1>",
        "alerts_index": ".opendistro-alerting-alerts",
        "alerts_history_index": ".opendistro-alerting-alert-history-write",
        "alerts_history_index_pattern": "<.opendistro-alerting-alert-history-{now/d}-1>",
        "comments_index": ".opensearch-alerting-comments-history-write",
        "comments_index_pattern": "<.opensearch-alerting-comments-history-{now/d}-1>",
        "query_index_mappings_by_type": {},
        "findings_enabled": false,
    })
}

impl Checked {
    /// Checks a put's body, fills in what the plugin fills in, and sets
    /// `type`, `last_update_time` and `enabled_time` as the plugin does;
    /// `replacing` is a put of an id that exists.
    pub(super) fn from_put(
        body: &Value,
        now_ms: i64,
        min_interval_secs: u64,
        replacing: bool,
    ) -> Result<Self, Refused> {
        let fields = body
            .as_object()
            .ok_or_else(|| illegal("the body must be an object"))?;
        let name = fields
            .get("name")
            .and_then(Value::as_str)
            .filter(|n| !n.trim().is_empty())
            .ok_or_else(|| illegal("Monitor name is null"))?;
        if name.chars().count() > MAX_NAME_CHARS {
            return Err(illegal(format!(
                "a monitor name is at most {MAX_NAME_CHARS} characters"
            )));
        }
        let kind = fields
            .get("monitor_type")
            .map_or(Ok("query_level_monitor"), |v| {
                v.as_str()
                    .ok_or_else(|| illegal("[monitor_type] must be a string"))
            })?;
        if !KINDS.contains(&kind) {
            return Err(Refused {
                status: 500,
                reason: "Monitor type should follow pattern [a-zA-Z0-9_]{5,25}".to_owned(),
                cause: Some(
                    "java.lang.IllegalStateException: Monitor type should follow pattern \
                     [a-zA-Z0-9_]{5,25}"
                        .to_owned(),
                ),
            });
        }
        let enabled = fields.get("enabled").map_or(Ok(true), |v| {
            v.as_bool()
                .ok_or_else(|| illegal("[enabled] must be true or false"))
        })?;
        let schedule = schedule_of(body, min_interval_secs)?;
        // ALLOC: the body as a map, at most the body bound, once a put.
        let mut stored = fields.clone();
        normalize_triggers(&mut stored)?;
        if let Some(inputs) = stored.get_mut("inputs").and_then(Value::as_array_mut) {
            normalize_inputs(inputs)?;
        }
        stored.insert("type".to_owned(), json!("monitor"));
        stored.insert("monitor_type".to_owned(), json!(kind));
        stored.insert("enabled".to_owned(), json!(enabled));
        stored.insert("last_update_time".to_owned(), json!(now_ms));
        stored.insert(
            "enabled_time".to_owned(),
            if enabled { json!(now_ms) } else { Value::Null },
        );
        // The plugin writes the schema it has when a monitor is replaced.
        stored.insert(
            "schema_version".to_owned(),
            json!(if replacing { 9 } else { 0 }),
        );
        stored.entry("inputs").or_insert_with(|| json!([]));
        stored.entry("triggers").or_insert_with(|| json!([]));
        stored.entry("data_sources").or_insert_with(data_sources);
        stored
            .entry("delete_query_index_in_every_run")
            .or_insert_with(|| json!(false));
        stored
            .entry("should_create_single_alert_for_findings")
            .or_insert_with(|| json!(false));
        stored.insert("owner".to_owned(), json!("alerting"));
        Ok(Self {
            stored: Value::Object(stored),
            enabled,
            schedule,
        })
    }
}
