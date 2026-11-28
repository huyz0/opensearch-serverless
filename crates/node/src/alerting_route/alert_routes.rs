//! `GET /monitors/alerts` and `POST /monitors/{id}/_acknowledge/alerts`:
//! the alerts a tenant's monitors have open, read from the monitors' alert
//! registers.

use std::cmp::Ordering;

use oss_ext::doc_sink::Doc;
use oss_meta::{Alerts, MetaError};
use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Value, json};

use super::alert::{self, Outcome, Subject, history_doc};
use super::notify;
use super::{AlertingService, alerting, bad, failed, now_millis, status_cause};
use crate::route_util::read_body;

/// The longest acknowledge body.
const MAX_REQUEST_BYTES: usize = 1 << 20;
/// The index a completed alert is written to.
const HISTORY_INDEX: &str = ".opendistro-alerting-alert-history-write";
/// The most alerts one answer holds.
const MAX_PAGE: usize = 10_000;

fn param(req: &Request<'_>, name: &str) -> Option<String> {
    req.param(name).map(std::borrow::Cow::into_owned)
}

fn number(req: &Request<'_>, name: &str, default: usize) -> Result<usize, RestError> {
    param(req, name).map_or(Ok(default), |v| {
        v.parse::<usize>()
            .map_err(|_| bad(format!("[{name}] must be a non-negative integer")))
    })
}

/// The alert field a sort string names: `monitor_name.keyword` and the like.
fn sort_key(alert: &Value, sort: &str) -> Value {
    let field = sort.strip_suffix(".keyword").unwrap_or(sort);
    alert.get(field).cloned().unwrap_or(Value::Null)
}

fn compare(a: &Value, b: &Value) -> Ordering {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => a
            .as_f64()
            .partial_cmp(&b.as_f64())
            .unwrap_or(Ordering::Equal),
        (Value::String(a), Value::String(b)) => a.cmp(b),
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Null, _) => Ordering::Greater,
        (_, Value::Null) => Ordering::Less,
        _ => Ordering::Equal,
    }
}

impl AlertingService {
    /// The tenant's alert set of one monitor.
    pub(super) fn alert_set(&self, req: &Request<'_>, monitor: &str) -> Result<Alerts, RestError> {
        let tenant = req
            .header("securitytenant")
            .unwrap_or(super::DEFAULT_TENANT);
        self.alert_set_of(tenant, monitor)
    }

    /// The alert set of `monitor` of `tenant`.
    pub(super) fn alert_set_of(&self, tenant: &str, monitor: &str) -> Result<Alerts, RestError> {
        Alerts::new(&self.store, &self.base, tenant, monitor).map_err(|e| failed(&e))
    }

    /// Records a run's outcome for a stored monitor in its alert register:
    /// completed alerts go to the history index first, the set is swapped,
    /// then the actions due are sent (at most once each: a send is recorded
    /// before it is made, and a failed one is recorded as an ERROR).
    pub(super) fn record_run(
        &self,
        tenant: &str,
        subject: &Subject<'_>,
        outcomes: &[Outcome<'_>],
        run: (&Value, &Value, &str),
    ) -> Result<(), RestError> {
        let (monitor, ctx, execution) = run;
        let now = now_millis();
        let set = self.alert_set_of(tenant, subject.id)?;
        let mut sends = Vec::new();
        set.change(|current| {
            let alerts = alert::parse(current)
                .map_err(|_| MetaError::InvalidArgument("a stored alert set is unreadable"))?;
            let moved = alert::after_run(&alerts, subject, outcomes, now, execution);
            let mut next = moved.clone().unwrap_or_else(|| alerts.clone());
            if let Some(sink) = &self.history {
                let done = alert::completed(&alerts, &next, now, tenant);
                if !done.is_empty() {
                    let docs: Vec<Doc> = done.iter().map(history_doc).collect();
                    let results = sink
                        .index(HISTORY_INDEX, &docs)
                        .map_err(|_| MetaError::InvalidArgument("alert history is not writable"))?;
                    if results.iter().any(|r| r.status >= 300) {
                        return Err(MetaError::InvalidArgument("alert history is not writable"));
                    }
                }
            }
            let (due, stamped) = if self.sender.is_some() {
                notify::due(&mut next, monitor, ctx, now)
            } else {
                (Vec::new(), false)
            };
            sends = due;
            Ok((moved.is_some() || stamped).then(|| alert::write(next)))
        })
        .map(|_| ())
        .map_err(|e| failed(&e))?;
        self.deliver(&set, sends, now)
    }

    /// Sends `sends`; the ones that fail leave their alert in ERROR.
    fn deliver(
        &self,
        set: &Alerts,
        sends: Vec<notify::Dispatch>,
        now: i64,
    ) -> Result<(), RestError> {
        let Some(sender) = &self.sender else {
            return Ok(());
        };
        let bad: Vec<(notify::Dispatch, String)> = sends
            .into_iter()
            .filter_map(|s| {
                sender
                    .send(&s.destination, &s.text)
                    .err()
                    .map(|why| (s, why))
            })
            .collect();
        if bad.is_empty() {
            return Ok(());
        }
        set.change(|current| {
            let mut alerts = alert::parse(current)
                .map_err(|_| MetaError::InvalidArgument("a stored alert set is unreadable"))?;
            let mut any = false;
            for (send, why) in &bad {
                any |= notify::failed(&mut alerts, send, why, now);
            }
            Ok(any.then(|| alert::write(alerts)))
        })
        .map(|_| ())
        .map_err(|e| failed(&e))
    }

    /// `GET /monitors/alerts`.
    pub(crate) fn alerts(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let m = self.monitors(req)?;
        let ids: Vec<String> = match param(req, "monitorId") {
            Some(one) => vec![one],
            None => m
                .index()
                .map_err(|e| failed(&e))?
                .iter()
                .map(|(id, _)| id.to_owned())
                .collect(),
        };
        let state = param(req, "alertState").unwrap_or_else(|| "ALL".to_owned());
        let severity = param(req, "severityLevel").unwrap_or_else(|| "ALL".to_owned());
        let search = param(req, "searchString")
            .unwrap_or_default()
            .to_lowercase();
        let sort = param(req, "sortString").unwrap_or_else(|| "monitor_name.keyword".to_owned());
        let descending = param(req, "sortOrder").is_some_and(|o| o.eq_ignore_ascii_case("desc"));
        let size = number(req, "size", 20)?.min(MAX_PAGE);
        let start = number(req, "startIndex", 0)?;
        let mut all: Vec<Value> = Vec::new();
        for id in &ids {
            let set = self.alert_set(req, id)?.read().map_err(|e| failed(&e))?;
            all.extend(
                alert::parse(&set)
                    .map_err(|_| alerting(500, "a stored alert set is unreadable", None))?,
            );
        }
        all.retain(|a| {
            (state == "ALL" || a["state"] == state.as_str())
                && (severity == "ALL" || a["severity"] == severity.as_str())
                && (search.is_empty() || a.to_string().to_lowercase().contains(&search))
        });
        all.sort_by(|a, b| {
            let c = compare(&sort_key(a, &sort), &sort_key(b, &sort));
            let c = if descending { c.reverse() } else { c };
            c.then_with(|| a["id"].as_str().cmp(&b["id"].as_str()))
        });
        let total = all.len();
        let page: Vec<Value> = all.into_iter().skip(start).take(size).collect();
        let body = json!({"alerts": page, "totalAlerts": total});
        resp.send(200, JSON, body.to_string().as_bytes())
    }

    /// `POST /monitors/{id}/_acknowledge/alerts`.
    pub(crate) fn acknowledge(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let raw = read_body(
            req,
            MAX_REQUEST_BYTES,
            "an acknowledge is over its size bound",
        )?;
        let body: Value =
            serde_json::from_slice(&raw).map_err(|e| bad(format!("invalid body: {e}")))?;
        let ids: Vec<String> = body["alerts"]
            .as_array()
            .ok_or_else(|| bad("an acknowledge lists its [alerts]"))?
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect();
        let (id, found) = self.load(req)?;
        if found.is_none() {
            let reason = format!("No monitor found with id [{id}]");
            return Err(alerting(
                500,
                reason.clone(),
                Some(format!(
                    "org.opensearch.ResourceNotFoundException: {reason}"
                )),
            ));
        }
        let now = now_millis();
        let mut result = None;
        self.alert_set(req, &id)?
            .change(|current| {
                let alerts = alert::parse(current)
                    .map_err(|_| MetaError::InvalidArgument("a stored alert set is unreadable"))?;
                let done = alert::acknowledge(&alerts, &ids, now);
                let write = done.changed.then(|| alert::write(done.alerts.clone()));
                result = Some(done);
                Ok(write)
            })
            .map_err(|e| failed(&e))?;
        let done = result.ok_or_else(|| alerting(500, "an acknowledge ran nothing", None))?;
        let failed_list: Vec<Value> = done
            .failed
            .iter()
            .map(|(id, why)| json!({ id: {"failed_reason": why} }))
            .collect();
        let _ = status_cause;
        let answer = json!({"success": done.success, "failed": failed_list});
        resp.send(200, JSON, answer.to_string().as_bytes())
    }
}
