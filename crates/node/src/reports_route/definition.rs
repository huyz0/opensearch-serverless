//! What a report definition must hold, checked against the stock plugin's
//! shape: `report_params`, `delivery` and `trigger`.

use oss_rest::RestError;
use serde_json::{Map, Value};

use super::cron;
use crate::route_util::refuse;

/// The report sources the plugin knows.
const SOURCES: &[&str] = &["Dashboard", "Visualization", "Saved search", "Notebook"];
/// The report formats the plugin knows.
const FORMATS: &[&str] = &["pdf", "png", "csv", "xlsx"];
/// The trigger types the plugin knows.
const TRIGGERS: &[&str] = &["Download", "On demand", "Schedule"];
/// The longest report name.
const MAX_NAME_BYTES: usize = 256;
/// The shortest interval a schedule may name, in seconds.
pub(super) const MIN_INTERVAL_SECS: i64 = 300;

fn bad(reason: impl Into<String>) -> RestError {
    refuse(400, "illegal_argument_exception", reason)
}

fn object<'a>(
    parent: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a Map<String, Value>, RestError> {
    parent
        .get(key)
        .and_then(Value::as_object)
        .ok_or_else(|| bad(format!("[{key}] is required and must be an object")))
}

fn text<'a>(parent: &'a Map<String, Value>, key: &str, within: &str) -> Result<&'a str, RestError> {
    parent
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| bad(format!("[{within}.{key}] is required and must be a string")))
}

fn one_of(value: &str, allowed: &[&str], within: &str) -> Result<(), RestError> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(bad(format!(
            "[{within}] must be one of {allowed:?}, not [{value}]"
        )))
    }
}

/// Checks a definition body.
///
/// # Errors
///
/// 400 naming the failing key; 501 `renderer_not_configured` for a PDF or
/// PNG, which needs a renderer this node has none of; 501
/// `delivery_unsupported` for a delivery to notification channels.
pub(super) fn check(body: &Value) -> Result<(), RestError> {
    let Some(top) = body.as_object() else {
        return Err(bad("the body must be a JSON object"));
    };
    let format = params(object(top, "report_params")?)?;
    let delivers = match top.get("delivery") {
        Some(d) => delivery_of(d)?,
        None => false,
    };
    trigger(object(top, "trigger")?)?;
    if matches!(format, "pdf" | "png") {
        return Err(refuse(
            501,
            "renderer_not_configured",
            format!("[{format}] needs a renderer and none is configured"),
        ));
    }
    if delivers {
        return Err(refuse(
            501,
            "delivery_unsupported",
            "delivery to notification channels is not served yet",
        ));
    }
    Ok(())
}

fn params(p: &Map<String, Value>) -> Result<&str, RestError> {
    let name = text(p, "report_name", "report_params")?;
    if name.is_empty() || name.len() > MAX_NAME_BYTES {
        return Err(bad(format!(
            "[report_params.report_name] must be 1 to {MAX_NAME_BYTES} bytes"
        )));
    }
    one_of(
        text(p, "report_source", "report_params")?,
        SOURCES,
        "report_params.report_source",
    )?;
    let core = object(p, "core_params")?;
    let base = text(core, "base_url", "report_params.core_params")?;
    if !base.starts_with('/') || base.starts_with("//") {
        return Err(bad(
            "[report_params.core_params.base_url] must be a path on this deployment",
        ));
    }
    let format = text(core, "report_format", "report_params.core_params")?;
    one_of(format, FORMATS, "report_params.core_params.report_format")?;
    if let Some(d) = core.get("time_duration").and_then(Value::as_str)
        && !d.starts_with('P')
    {
        return Err(bad(
            "[report_params.core_params.time_duration] must be an ISO-8601 duration",
        ));
    }
    Ok(format)
}

/// Whether the delivery names a channel.
fn delivery_of(d: &Value) -> Result<bool, RestError> {
    let Some(d) = d.as_object() else {
        return Err(bad("[delivery] must be an object"));
    };
    match d.get("configIds") {
        None => Ok(false),
        Some(Value::Array(ids)) if ids.iter().all(Value::is_string) => Ok(!ids.is_empty()),
        Some(_) => Err(bad("[delivery.configIds] must be an array of strings")),
    }
}

fn trigger(t: &Map<String, Value>) -> Result<(), RestError> {
    let kind = text(t, "trigger_type", "trigger")?;
    one_of(kind, TRIGGERS, "trigger.trigger_type")?;
    if kind != "Schedule" {
        return Ok(());
    }
    let params = object(t, "trigger_params")?;
    let schedule = object(params, "schedule")?;
    match text(params, "schedule_type", "trigger.trigger_params")? {
        "Recurring" => interval(object(schedule, "interval")?),
        "Cron based" => {
            let c = object(schedule, "cron")?;
            if !cron::valid(text(
                c,
                "expression",
                "trigger.trigger_params.schedule.cron",
            )?) {
                return Err(bad(
                    "[trigger.trigger_params.schedule.cron.expression] is not a five-field cron expression",
                ));
            }
            text(c, "timezone", "trigger.trigger_params.schedule.cron").map(|_| ())
        }
        other => Err(bad(format!(
            "[trigger.trigger_params.schedule_type] must be [Recurring] or [Cron based], not [{other}]"
        ))),
    }
}

fn interval(i: &Map<String, Value>) -> Result<(), RestError> {
    let period = i.get("period").and_then(Value::as_i64).filter(|p| *p > 0);
    let Some(period) = period else {
        return Err(bad(
            "[trigger.trigger_params.schedule.interval.period] must be a positive integer",
        ));
    };
    let unit = match text(i, "unit", "trigger.trigger_params.schedule.interval")? {
        "MINUTES" => 60,
        "HOURS" => 3_600,
        "DAYS" => 86_400,
        other => {
            return Err(bad(format!(
                "[trigger.trigger_params.schedule.interval.unit] must be MINUTES, HOURS or DAYS, not [{other}]"
            )));
        }
    };
    if period.saturating_mul(unit) < MIN_INTERVAL_SECS {
        return Err(bad(format!(
            "[trigger.trigger_params.schedule.interval] must be at least {MIN_INTERVAL_SECS} seconds"
        )));
    }
    Ok(())
}
