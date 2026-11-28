//! `GET /detectors/{id}/_profile[/{type}]`: where a detector is in its life,
//! from its stored job and the clock alone.
//!
//! The state is arithmetic: a detector whose job is off is `DISABLED`; one
//! that is on is `INIT` until the first slot after it was enabled is due
//! (no run has been offered yet) and `RUNNING` after. No claim register is
//! read and no model is asked, so a profile is the detector's document and
//! nothing else, whichever node answers; a failed run's `error`, the model
//! list and the entity counts need a run record the runner does not write
//! yet and are not answered.

use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Value, json};

use super::slots::{Timing, latest_due};
use super::{AdService, bad, not_found, now_millis, windows};
use crate::route_util::refuse;

/// The profile types this node answers.
const TYPES: [&str; 1] = ["state"];

/// The state of a detector at `now_ms`.
pub(super) fn state(detector: &Value, job: &Value, now_ms: i64) -> Result<&'static str, String> {
    if job["enabled"].as_bool() != Some(true) {
        return Ok("DISABLED");
    }
    let period = windows::interval_ms(detector)?;
    let enabled = job["enabled_time"].as_i64().unwrap_or(now_ms);
    let first = enabled
        .checked_div(period)
        .and_then(|s| u64::try_from(s).ok());
    Ok(match (latest_due(now_ms, period, Timing::DEFAULT), first) {
        (Some(due), Some(first)) if due >= first => "RUNNING",
        _ => "INIT",
    })
}

impl AdService {
    /// `GET /detectors/{id}/_profile` and `/_profile/{type}`.
    pub(crate) fn profile(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let id = Self::id_of(req)?;
        if let Some(kind) = req.path_param("type") {
            let wanted = kind.split(',').map(str::trim);
            for one in wanted {
                if !TYPES.contains(&one) {
                    return Err(refuse(
                        400,
                        "illegal_argument_exception",
                        format!(
                            "the profile type [{one}] is not served; this node answers {TYPES:?}"
                        ),
                    ));
                }
            }
        }
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let stored = self
            .stored(&id, &mut marker, &mut doc)?
            .ok_or_else(|| not_found(&id))?;
        let now = i64::try_from(now_millis()).unwrap_or(i64::MAX);
        let state = state(&stored["detector"], &stored["job"], now).map_err(bad)?;
        resp.send(200, JSON, json!({"state": state}).to_string().as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: i64 = 60_000;

    fn detector() -> Value {
        json!({"detection_interval": {"period": {"interval": 10, "unit": "Minutes"}}})
    }

    #[test]
    fn a_job_that_is_off_is_disabled() {
        assert_eq!(
            state(&detector(), &json!({"enabled": false}), 0),
            Ok("DISABLED")
        );
        assert_eq!(state(&detector(), &Value::Null, 0), Ok("DISABLED"));
    }

    #[test]
    fn an_enabled_job_is_init_until_a_slot_since_it_was_enabled_is_due() {
        let job = json!({"enabled": true, "enabled_time": 25 * MINUTE});
        let at = |m: i64| state(&detector(), &job, m * MINUTE);
        // enabled in slot 2 (20 to 30 minutes); it ends at 30, due 7 seconds later
        assert_eq!(at(25), Ok("INIT"));
        assert_eq!(at(30), Ok("INIT"));
        assert_eq!(at(31), Ok("RUNNING"));
        assert_eq!(at(120), Ok("RUNNING"));
    }
}
