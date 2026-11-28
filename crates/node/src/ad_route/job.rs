//! `POST /detectors/{id}/_start` and `_stop`: the detector's job record.
//!
//! The job record (`enabled`, `enabled_time`, `disabled_time`, `schedule`)
//! sits in the detector's own document, so a start is the document's read
//! and its replacement: the detector set's marker is the only register
//! touched, and nothing lists. A start then names the tenant in the
//! `ad` schedule registry, after the write, so a runner that finds the entry
//! finds the enabled job. A stop leaves the entry: a stale entry costs one
//! empty read a refresh, and the runner that finds no enabled detector
//! removes it.

use std::sync::Arc;

use oss_meta::ScheduleRegistry;
use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Value, json};

use super::{AdService, bad, failed, not_found, now_millis, windows};
use crate::route_util::{read_body, refuse};

use super::run::{NAMESPACE, TENANT};

impl AdService {
    fn registry(&self) -> Result<ScheduleRegistry, RestError> {
        ScheduleRegistry::in_namespace(Arc::clone(&self.store), &self.base, NAMESPACE)
            .map_err(|e| failed(&e))
    }

    /// `POST /detectors/{id}/_start`.
    pub(crate) fn start(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        self.flip(req, resp, true)
    }

    /// `POST /detectors/{id}/_stop`.
    pub(crate) fn stop(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        self.flip(req, resp, false)
    }

    fn flip(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
        enable: bool,
    ) -> Result<(), RestError> {
        let id = Self::id_of(req)?;
        let raw = read_body(req, 1 << 10, "the body is over the limit for a start")?;
        if !raw.is_empty() {
            let body: Value =
                serde_json::from_slice(&raw).map_err(|e| bad(format!("invalid body: {e}")))?;
            if body.get("start_time").is_some() || body.get("end_time").is_some() {
                return Err(refuse(
                    400,
                    "illegal_argument_exception",
                    "a historical analysis is not served yet",
                ));
            }
        }
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let current = self
            .stored(&id, &mut marker, &mut doc)?
            .ok_or_else(|| not_found(&id))?;
        let seq = current["seq_no"].as_i64().unwrap_or(0);
        let detector = &current["detector"];
        let now = i64::try_from(now_millis()).unwrap_or(i64::MAX);
        let before = &current["job"];
        let was = before["enabled"].as_bool().unwrap_or(false);
        let job = if enable && was {
            before.clone()
        } else {
            let period = windows::interval_ms(detector)
                .map_err(bad)?
                .checked_div(60_000)
                .unwrap_or(1);
            json!({
                "name": id,
                "enabled": enable,
                "enabled_time": if enable { json!(now) } else { before["enabled_time"].clone() },
                "disabled_time": if enable { Value::Null } else { json!(now) },
                "schedule": {"interval": {"start_time": now, "period": period, "unit": "Minutes"}},
            })
        };
        let changed = enable != was;
        let next = if changed { seq.saturating_add(1) } else { seq };
        if changed {
            self.write(&id, next, detector, &job, false, &mut marker)?;
        }
        if enable {
            self.registry()?.register(TENANT).map_err(|e| failed(&e))?;
            let body = json!({
                "_id": id, "_version": next.saturating_add(1),
                "_seq_no": next, "_primary_term": 1,
            });
            return resp.send(200, JSON, body.to_string().as_bytes());
        }
        resp.send(
            200,
            JSON,
            json!(format!("Stopped detector: {id}"))
                .to_string()
                .as_bytes(),
        )
    }
}
