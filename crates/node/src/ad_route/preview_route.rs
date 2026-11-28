//! `POST /detectors/_preview` and `POST /detectors/{id}/_preview`.

use std::sync::Arc;

use oss_format::DocKind;
use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Value, json};

use super::{AdService, bad, fresh_id, not_found, now_millis, preview, windows};
use crate::route_util::{read_body, refuse};

impl AdService {
    /// A service whose previews read their windows through `search`.
    #[must_use]
    pub fn with_search(mut self, search: Arc<dyn super::WindowSearch>) -> Self {
        self.search = Some(search);
        self
    }

    /// The preview of a stored detector, or of the one the body carries.
    pub(crate) fn preview(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let max = DocKind::AdDetectors.max_doc_bytes();
        let raw = read_body(req, max, "the body is over the limit for a preview")?;
        let body: Value =
            serde_json::from_slice(&raw).map_err(|e| bad(format!("invalid body: {e}")))?;
        let millis = |name: &str| {
            body[name]
                .as_i64()
                .ok_or_else(|| bad(format!("[{name}] is required, in epoch milliseconds")))
        };
        let (start, end) = (millis("period_start")?, millis("period_end")?);
        if end <= start {
            return Err(bad("[period_end] must be after [period_start]"));
        }
        let detector = if let Some(id) = req.path_param("id") {
            let mut marker = self.marker(req)?;
            let mut doc = Self::doc_buffer(req)?;
            let stored = self
                .stored(&id, &mut marker, &mut doc)?
                .ok_or_else(|| not_found(&id))?;
            stored["detector"].clone()
        } else {
            let given = body
                .get("detector")
                .ok_or_else(|| bad("[detector] is required"))?;
            super::detector::checked(given, now_millis(), &mut fresh_id).map_err(bad)?
        };
        let search = self.search.as_ref().ok_or_else(|| {
            refuse(
                500,
                "ad_source_unavailable",
                "this node cannot search for a preview",
            )
        })?;
        let features = windows::features(&detector).map_err(bad)?;
        let params = preview::params(&detector, features.len()).map_err(bad)?;
        let interval = windows::interval_ms(&detector).map_err(bad)?;
        let whole = end
            .saturating_sub(start)
            .saturating_add(interval.saturating_sub(1))
            .checked_div(interval)
            .unwrap_or(0);
        let count = usize::try_from(whole).unwrap_or(usize::MAX);
        let plan = preview::plan(start, interval, count, &params).map_err(bad)?;
        let categories = windows::categories(&detector);
        let entities = if categories.is_empty() {
            1
        } else {
            windows::MAX_ENTITIES
        };
        // The model, its workspace and the windows, the search answer and the result are held until the answer is written.
        req.body().reserve(preview::reserved(&params, entities))?;
        let indices: Vec<String> = detector["indices"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|i| i.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let request = windows::request(&detector, &plan.all).map_err(bad)?;
        let answer = search.search(&indices, &request)?;
        let span = windows::Span {
            from_ms: plan.all.start_of(plan.history),
            interval_ms: interval,
            count,
        };
        let mut results = Vec::new();
        if categories.is_empty() {
            let values = windows::parse(&answer, &detector, &plan.all).map_err(bad)?;
            let scored = preview::score(params, &values, plan.history).map_err(bad)?;
            let range = values.get(plan.history..).unwrap_or_default();
            results = preview::results(&features, &span, range, &scored, None);
        } else {
            let found = windows::parse_entities(&answer, &detector, &plan.all).map_err(bad)?;
            for entity in &found {
                let scored =
                    preview::score(params.clone(), &entity.windows, plan.history).map_err(bad)?;
                let range = entity.windows.get(plan.history..).unwrap_or_default();
                results.extend(preview::results(
                    &features,
                    &span,
                    range,
                    &scored,
                    Some(&entity.key),
                ));
            }
        }
        let out = json!({"anomaly_result": results, "anomaly_detector": detector});
        resp.send(200, JSON, out.to_string().as_bytes())
    }
}
