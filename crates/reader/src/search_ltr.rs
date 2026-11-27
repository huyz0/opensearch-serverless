//! An `sltr` rescore on one shard: the learning to rank plugin's
//! `LtrRescorer` over a stored model.
//!
//! Each feature's template renders with the query's `params`, parses as a
//! query and is scored against the window's hits; the vector of scores, 0
//! for a feature that did not match, goes to the model, and the model's score
//! joins the first pass's as a rescore query's does.

use std::collections::HashMap;
use std::sync::Arc;

use lucene_search::BooleanQuery;
use lucene_search::collector::{TotalHits, TotalHitsRelation};
use lucene_search::rescorer::{QueryRescoreMode, QueryRescorer, Rescorer as _};
use lucene_search::top_docs::{ShardScoreDoc, TopDocs};
use lucene_search::values_source::ValuesContext;
use oss_blob::BlobStore;

use crate::OpenShard;
use crate::dsl::queries::sltr::Sltr;
use crate::dsl::query::parse_inner;
use crate::dsl::rescore::Rescorer;
use crate::failure::Failure;
use crate::ltr::models::{Feature, ResolveError, Resolved, installed};
use crate::lucene_query::{QueryContext, shard_failure, to_lucene};
use crate::search::ShardError;

/// The most bytes a rendered feature template may take.
const RENDER_LIMIT: usize = 64 << 10;

/// One rescorer, its model resolved and its feature queries built.
pub(crate) struct Built {
    window: usize,
    query_weight: f32,
    rescore_query_weight: f32,
    mode: QueryRescoreMode,
    model: Arc<Resolved>,
    /// One per feature of the set; `None` for one not active.
    queries: Vec<Option<BooleanQuery>>,
}

fn refused(status: u16, kind: &'static str, reason: String) -> ShardError {
    ShardError {
        status,
        failure: Failure::plain(kind, reason),
    }
}

fn bad(reason: String) -> ShardError {
    refused(400, "illegal_argument_exception", reason)
}

/// Resolves the model and builds the feature queries of `r`'s `sltr`.
pub(crate) fn build<S: BlobStore + 'static>(
    r: &Rescorer,
    q: &Sltr,
    cx: &QueryContext<'_>,
    shard: &OpenShard<S>,
    index: &str,
) -> Result<Built, ShardError> {
    if q.featureset.is_some() {
        return Err(bad(
            // ALLOC: error path; one message.
            "an [sltr] rescore needs a [model], not a [featureset]".to_owned(),
        ));
    }
    let Some(models) = installed() else {
        return Err(bad(
            // ALLOC: error path; one message.
            "an [sltr] query needs serverless.ltr.enabled on this node".to_owned(),
        ));
    };
    if r.window > models.max_window() {
        // ALLOC: one message or window copy a request, bounded by the request's window.
        return Err(bad(format!(
            // ALLOC: error path; one message.
            "the window [{}] of an [sltr] rescore is over serverless.ltr.max_window [{}]",
            r.window,
            models.max_window()
        )));
    }
    let model = models
        .resolve(q.store.as_deref(), &q.model)
        .map_err(|e| match e {
            ResolveError::NotFound(m) => refused(404, "resource_not_found_exception", m),
            ResolveError::TooBig(m) => refused(429, "circuit_breaking_exception", m),
            ResolveError::Failed(m) => refused(500, "ltr_store_failure", m),
        })?;
    let queries = feature_queries(&model.features, (q, &q.model), cx, shard, index)?;
    Ok(Built {
        window: r.window,
        query_weight: r.query_weight,
        rescore_query_weight: r.rescore_query_weight,
        mode: r.mode,
        model,
        queries,
    })
}

/// The query of each of `features` rendered with `q`'s params and built;
/// `None` for one `q` does not activate. `what` names the model or feature
/// set for a message.
pub(crate) fn feature_queries<S: BlobStore + 'static>(
    features: &[Feature],
    (q, what): (&Sltr, &str),
    cx: &QueryContext<'_>,
    shard: &OpenShard<S>,
    index: &str,
) -> Result<Vec<Option<BooleanQuery>>, ShardError> {
    if let Some(active) = &q.active_features
        && let Some(unknown) = active
            .iter()
            .find(|a| !features.iter().any(|f| &f.name == *a))
    {
        // ALLOC: one message or window copy a request, bounded by the request's window.
        return Err(bad(format!(
            // ALLOC: error path; one message.
            "unknown feature [{unknown}] in active_features of [{what}]"
        )));
    }
    // ALLOC: one entry a feature of the set, at most MAX_FEATURES.
    let mut queries = Vec::with_capacity(features.len());
    for feature in features {
        let active = q
            .active_features
            .as_ref()
            .is_none_or(|a| a.contains(&feature.name));
        if !active {
            queries.push(None);
            continue;
        }
        if let Some(missing) = feature.params.iter().find(|n| q.params.get(n).is_none()) {
            // ALLOC: one message or window copy a request, bounded by the request's window.
            return Err(bad(format!(
                // ALLOC: error path; one message.
                "missing required param [{missing}] for feature [{}]",
                feature.name
            )));
        }
        let text = crate::template::render(&feature.template, &q.params, RENDER_LIMIT)
            // ALLOC: one message or window copy a request, bounded by the request's window.
            .map_err(|e| bad(format!("feature [{}] does not render: {e:?}", feature.name)))?;
        let rec = crate::ltr::json::record(&text)
            // ALLOC: one message or window copy a request, bounded by the request's window.
            .map_err(|e| bad(format!("feature [{}] is not a query: {e}", feature.name)))?;
        let mut p = rec.parser();
        let parsed = parse_inner(&mut p).map_err(|e| ShardError::of_query(e.into_failure()))?;
        let built = to_lucene(&parsed, cx)
            .map_err(|f| ShardError::of_query(shard_failure(f, index, &shard.meta.uuid)))?;
        queries.push(Some(built));
    }
    Ok(queries)
}

impl Built {
    /// Whether a feature query needs the points open.
    pub(crate) fn has_points(&self) -> bool {
        self.queries.iter().flatten().any(crate::search::has_points)
    }

    /// The window of this rescorer.
    pub(crate) fn window(&self) -> usize {
        self.window
    }

    /// Rescores `hits`, best first, in place.
    pub(crate) fn apply(
        &self,
        ctx: &ValuesContext<'_>,
        hits: &mut [ShardScoreDoc],
    ) -> Result<(), ShardError> {
        let top = hits.len().min(self.window);
        if top == 0 {
            return Ok(());
        }
        let vectors = vectors(ctx, hits.get(..top).unwrap_or_default(), &self.queries)?;
        let width = self.queries.len();
        // ALLOC: one row, reused for every hit of the window.
        let mut row = vec![0.0f32; width];
        for (i, hit) in hits.iter_mut().take(top).enumerate() {
            let seen = vectors
                .get(i.saturating_mul(width)..i.saturating_add(1).saturating_mul(width))
                .unwrap_or_default();
            for (cell, v) in row.iter_mut().zip(seen) {
                *cell = if v.is_nan() { 0.0 } else { *v };
            }
            let model = self.model.evaluator.score(&row);
            hit.score = self.mode.combine(
                hit.score * self.query_weight,
                model * self.rescore_query_weight,
            );
        }
        for hit in hits.iter_mut().skip(top) {
            hit.score *= self.query_weight;
        }
        hits.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.doc.cmp(&b.doc)));
        Ok(())
    }
}

/// Each feature's score of each of `window`'s hits, a hit's row after a
/// hit's row; NaN where a feature is not active or did not match.
pub(crate) fn vectors(
    ctx: &ValuesContext<'_>,
    window: &[ShardScoreDoc],
    queries: &[Option<BooleanQuery>],
) -> Result<Vec<f32>, ShardError> {
    let top = window.len();
    let first_pass = TopDocs {
        total_hits: TotalHits {
            value: 0,
            relation: TotalHitsRelation::EqualTo,
        },
        // ALLOC: one copy of the window, once.
        score_docs: window.to_vec(),
    };
    // ALLOC: one entry a hit of the window, at most the window.
    let slot: HashMap<i32, usize> = window.iter().enumerate().map(|(i, h)| (h.doc, i)).collect();
    let width = queries.len();
    // ALLOC: one vector of the window's hits times the features.
    let mut out = vec![f32::NAN; top.saturating_mul(width)];
    for (f, query) in queries.iter().enumerate() {
        let Some(query) = query else { continue };
        let rescorer = QueryRescorer::new(
            query.clone(),
            // ALLOC: one closure a feature.
            Arc::new(
                |_first: f32, matches: bool, second: f32| {
                    if matches { second } else { f32::NAN }
                },
            ),
        );
        let scored = rescorer
            .rescore(ctx, &first_pass, top)
            .map_err(ShardError::engine)?;
        for hit in scored.score_docs {
            if let Some(&i) = slot.get(&hit.doc)
                && let Some(v) = out.get_mut(i.saturating_mul(width).saturating_add(f))
            {
                *v = hit.score;
            }
        }
    }
    Ok(out)
}
