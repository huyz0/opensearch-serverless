//! `POST /monitors/{id}/_execute` and `POST /monitors/_execute`: one run of
//! a query-level monitor, as the plugin answers it. The input search goes
//! through the node's search; each trigger's condition runs in the Rust
//! Painless runtime, so a condition needs
//! `serverless.scripting.painless.native_compile` and reports a script error
//! while it is off. Nothing is stored: a run's alerts are a later slice.

use lucene_painless::ingest::run as run_script;
use lucene_painless::ir::Constant;
use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Map, Value, json};

use super::alert::{Outcome, Subject};
use super::monitor::Checked;
use super::{AlertingService, alerting, bad, now_millis, refused, status_cause};
use crate::route_util::read_body;

/// The longest body an execute takes.
const MAX_REQUEST_BYTES: usize = 1 << 20;
/// The key a wrapped condition leaves its answer under.
const ANSWER: &str = "alerting_result";

/// `java.time.Instant.toString` of a millisecond time.
// ARITH: civil-date arithmetic over a bounded day count; values stay far inside i64.
#[allow(clippy::arithmetic_side_effects)]
fn instant(millis: i64) -> String {
    let days = millis.div_euclid(86_400_000);
    let ms_of_day = millis.rem_euclid(86_400_000);
    // Howard Hinnant's civil-from-days.
    let z = days.saturating_add(719_468);
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    let (h, m, s, ms) = (
        ms_of_day / 3_600_000,
        ms_of_day / 60_000 % 60,
        ms_of_day / 1000 % 60,
        ms_of_day % 1000,
    );
    // ALLOC: one timestamp, a response field.
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}.{ms:03}Z")
}

/// The length of a monitor's period in milliseconds: its interval, or a
/// minute for a cron schedule, whose previous fire the node does not track.
fn period_millis(monitor: &Value) -> i64 {
    let period = &monitor["schedule"]["period"];
    let interval = period["interval"].as_i64().unwrap_or(1);
    let unit = match period["unit"].as_str() {
        Some("HOURS") => 3_600_000,
        Some("DAYS") => 86_400_000,
        _ => 60_000,
    };
    interval.saturating_mul(unit)
}

/// `value` with `{{period_start}}` and `{{period_end}}` in its strings
/// replaced by the period's epoch milliseconds, as the plugin renders them.
fn render(value: &mut Value, start: i64, end: i64) {
    match value {
        Value::String(text) => {
            for (name, ms) in [("period_start", start), ("period_end", end)] {
                for form in [format!("{{{{{name}}}}}"), format!("{{{{ {name} }}}}")] {
                    if text.contains(&form) {
                        *text = text.replace(&form, &ms.to_string());
                    }
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| render(v, start, end)),
        Value::Object(members) => members.values_mut().for_each(|v| render(v, start, end)),
        _ => {}
    }
}

fn constant(value: &Value) -> Constant {
    match value {
        Value::Null => Constant::Null,
        Value::Bool(b) => Constant::Bool(*b),
        Value::Number(n) => n.as_i64().map_or_else(
            || Constant::Double(n.as_f64().unwrap_or(0.0)),
            |i| i32::try_from(i).map_or(Constant::Long(i), Constant::Int),
        ),
        Value::String(s) => Constant::Str(s.clone()),
        // ALLOC: one run's working copy of the results, bounded by the search answer.
        Value::Array(items) => Constant::List(items.iter().map(constant).collect()),
        Value::Object(members) => Constant::Map(
            members
                .iter()
                // ALLOC: one run's working copy of the results, bounded by the search answer.
                .map(|(k, v)| (Constant::Str(k.clone()), constant(v)))
                .collect(),
        ),
    }
}

/// The error text the plugin gives a condition that failed.
fn script_error(source: &str, reason: &str) -> String {
    serde_json::to_string_pretty(&json!({
        "type": "script_exception",
        "reason": "runtime error",
        "script": source,
        "lang": "painless",
        "caused_by": {"type": "exception", "reason": reason},
    }))
    .unwrap_or_default()
}

/// Whether `source` holds, over `ctx`, or why it could not be decided.
fn condition(source: &str, ctx: &Constant) -> Result<bool, String> {
    if !oss_reader::painless_switch::native_compile() {
        return Err(script_error(
            source,
            "Painless is not compiled on this node (serverless.scripting.painless.native_compile is off)",
        ));
    }
    let expression = source.trim().trim_end_matches(';').trim();
    let wrapped = format!("ctx.{ANSWER} = ({expression});");
    let left = run_script(&wrapped, &Constant::Map(Vec::new()), ctx)
        .map_err(|e| script_error(source, &e.to_string()))?;
    let Constant::Map(entries) = left else {
        return Err(script_error(source, "the script left no result"));
    };
    entries
        .iter()
        .find_map(|(k, v)| match (k, v) {
            (Constant::Str(k), Constant::Bool(b)) if k == ANSWER => Some(*b),
            _ => None,
        })
        .ok_or_else(|| script_error(source, "a trigger condition returns a boolean"))
}

/// The reason a failed input search gives: the search error's own reason when
/// the node's answer carries one, else its whole text.
fn failure_reason(e: &RestError) -> String {
    let text = e.as_exception().map_or("the search failed", |x| x.reason());
    let marker = "\"reason\":\"";
    text.find(marker)
        .and_then(|at| text.get(at.saturating_add(marker.len())..))
        .and_then(|rest| rest.split('"').next())
        // ALLOC: error path; one message.
        .map_or_else(|| text.to_owned(), str::to_owned)
}

impl AlertingService {
    /// The monitor's input search over its indices, or the plugin's 500.
    fn input_results(
        &self,
        monitor: &Value,
        start: i64,
        end: i64,
    ) -> Result<Vec<Value>, RestError> {
        let Some(search) = monitor["inputs"].get(0).and_then(|i| i.get("search")) else {
            return Ok(Vec::new());
        };
        let Some(engine) = &self.search else {
            return Err(alerting(
                503,
                "this node has no search to run a monitor's input",
                None,
            ));
        };
        let indices: Vec<String> = search["indices"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let mut body = search["query"].clone();
        render(&mut body, start, end);
        let answer = engine.search(&indices, &body).map_err(|e| {
            let reason = failure_reason(&e);
            let cause = format!("org.opensearch.core.common.ParsingException: {reason}");
            alerting(500, reason, Some(cause))
        })?;
        Ok(vec![answer])
    }

    /// One run of `monitor` over the period ending at `end`.
    pub(super) fn run_monitor(
        &self,
        monitor: &Value,
        end: i64,
        dryrun: bool,
    ) -> Result<Value, RestError> {
        let kind = monitor["monitor_type"].as_str().unwrap_or_default();
        if kind != "query_level_monitor" {
            return Err(alerting(
                501,
                format!("a {kind} is not executed by this node yet"),
                None,
            ));
        }
        let start = end.saturating_sub(period_millis(monitor));
        let results = self.input_results(monitor, start, end)?;
        let mut triggers = Map::new();
        for trigger in monitor["triggers"].as_array().into_iter().flatten() {
            let Some(body) = trigger.get("query_level_trigger") else {
                continue;
            };
            let id = body["id"].as_str().unwrap_or_default();
            let source = body["condition"]["script"]["source"]
                .as_str()
                .unwrap_or_default();
            let ctx = constant(&json!({
                "results": results,
                "periodStart": instant(start),
                "periodEnd": instant(end),
                "monitor": monitor,
                "trigger": body,
                "alert": null,
                "error": null,
                "dryrun": dryrun,
            }));
            let (triggered, error) = match condition(source, &ctx) {
                Ok(b) => (b, Value::Null),
                // A condition that fails raises an alert in the ERROR state.
                Err(text) => (true, Value::String(text)),
            };
            triggers.insert(
                id.to_owned(),
                json!({
                    "name": body["name"],
                    "triggered": triggered,
                    "action_results": {},
                    "ppl_query_results": [],
                    "error": error,
                }),
            );
        }
        Ok(json!({
            "monitor_name": monitor["name"],
            "period_start": instant(start),
            "period_end": instant(end),
            "error": null,
            "input_results": {
                "results": results,
                "ppl_query_results": [],
                "ppl_num_results": null,
                "error": null,
            },
            "trigger_results": triggers,
        }))
    }

    /// The period end a request names, else now.
    fn period_end(req: &Request<'_>, body: &Value) -> Result<i64, RestError> {
        match body["period_end"].as_str() {
            None => Ok(now_millis()),
            Some(text) => oss_reader::date::parse_to_millis(text, false)
                .map_err(|_| bad(format!("failed to parse period_end [{text}]"))),
        }
        .inspect(|_| {
            let _ = req;
        })
    }

    fn execute_with(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
        monitor: Option<Value>,
        body: &Value,
    ) -> Result<(), RestError> {
        let dryrun = crate::route_util::flag(req, "dryrun", false);
        let end = Self::period_end(req, body)?;
        let (monitor, stored) = match monitor {
            Some(m) => (m, None),
            None => {
                let (id, found) = self.load(req)?;
                let Some((entry, stored)) = found else {
                    let reason = format!("Can't find monitor with id: {id}");
                    return Err(alerting(404, reason.clone(), status_cause(&reason)));
                };
                (stored, Some((id, entry.version)))
            }
        };
        let answer = self.run_monitor(&monitor, end, dryrun)?;
        if let (false, Some((id, version))) = (dryrun, &stored) {
            let tenant = req
                .header("securitytenant")
                .unwrap_or(super::DEFAULT_TENANT);
            self.record(tenant, &monitor, &answer, (id, *version))?;
        }
        resp.send(200, JSON, answer.to_string().as_bytes())
    }

    /// Opens, keeps or completes the alerts of the run `answer` describes.
    pub(super) fn record(
        &self,
        tenant: &str,
        monitor: &Value,
        answer: &Value,
        (id, version): (&str, i64),
    ) -> Result<(), RestError> {
        let results = answer["trigger_results"].as_object();
        let mut outcomes = Vec::new();
        for trigger in monitor["triggers"].as_array().into_iter().flatten() {
            let Some(body) = trigger.get("query_level_trigger") else {
                continue;
            };
            let tid = body["id"].as_str().unwrap_or_default();
            let Some(run) = results.and_then(|r| r.get(tid)) else {
                continue;
            };
            outcomes.push(Outcome {
                id: tid,
                name: body["name"].as_str().unwrap_or_default(),
                severity: body["severity"].as_str().unwrap_or("1"),
                triggered: run["triggered"].as_bool().unwrap_or(false),
                error: run["error"].as_str(),
            });
        }
        let subject = Subject {
            id,
            name: monitor["name"].as_str().unwrap_or_default(),
            version,
        };
        let execution = format!("{id}_{}", answer["period_end"].as_str().unwrap_or_default());
        self.record_run(tenant, &subject, &outcomes, &execution)
    }

    /// `POST /monitors/{id}/_execute`: a stored monitor.
    pub(crate) fn execute_stored(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let raw = read_body(req, MAX_REQUEST_BYTES, "an execute is over its size bound")?;
        let body: Value = if raw.iter().all(u8::is_ascii_whitespace) {
            Value::Null
        } else {
            serde_json::from_slice(&raw).map_err(|e| bad(format!("invalid body: {e}")))?
        };
        self.execute_with(req, resp, None, &body)
    }

    /// `POST /monitors/_execute`: the monitor in the body, stored nowhere.
    pub(crate) fn execute_body(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let raw = read_body(req, MAX_REQUEST_BYTES, "an execute is over its size bound")?;
        let body: Value =
            serde_json::from_slice(&raw).map_err(|e| bad(format!("invalid body: {e}")))?;
        let checked = Checked::from_put(&body, now_millis(), self.min_interval_secs, false)
            .map_err(refused)?;
        self.execute_with(req, resp, Some(checked.stored), &body)
    }
}
