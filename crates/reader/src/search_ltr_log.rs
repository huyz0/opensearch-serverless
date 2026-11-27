//! `ext.ltr_log` on one shard: the learning to rank plugin's
//! `LoggingFetchSubPhase`. Each log names an `sltr` query (by `_name`, or by
//! the position of a rescore); its features are scored against every hit of
//! the page and written under `fields._ltrlog`.

use std::collections::HashMap;

use lucene_search::BooleanQuery;
use lucene_search::field_norms::FieldNorms;
use lucene_search::index_searcher::IndexSearcher;
use lucene_search::multi_segment::OpenSegment;
use lucene_search::top_docs::ShardScoreDoc;
use lucene_search::values_source::ValuesContext;
use oss_blob::BlobStore;

use crate::OpenShard;
use crate::dsl::SearchSource;
use crate::dsl::ltr_log::LogSpec;
use crate::dsl::queries::More;
use crate::dsl::queries::sltr::Sltr;
use crate::dsl::query::Query;
use crate::failure::Failure;
use crate::fetch::docvalue_fields::{HitField, ValueKind};
use crate::ltr::models::{Feature, ResolveError, installed};
use crate::lucene_query::QueryContext;
use crate::search::{Hit, ShardError};
use crate::search_ltr::{feature_queries, vectors};

/// One log's feature values for every hit of the page.
#[derive(Clone, Debug, PartialEq)]
pub struct LogRows {
    /// The log's name.
    pub name: String,
    /// The features, in the set's order.
    pub features: Vec<String>,
    /// For each hit's global document id, a value a feature; NaN for none.
    pub rows: HashMap<i32, Vec<f32>>,
}

/// What `_ltrlog` is written from.
#[derive(Clone, Debug, PartialEq)]
pub struct LtrLogged {
    /// The logs, in the request's order.
    pub logs: Vec<LogRows>,
}

/// One log, its features resolved and their queries built.
struct One {
    name: String,
    names: Vec<String>,
    queries: Vec<Option<BooleanQuery>>,
    missing_as_zero: bool,
}

/// A search's logs.
pub(crate) struct Plan {
    logs: Vec<One>,
}

fn bad(reason: String) -> ShardError {
    ShardError::bad_request(Failure::plain("illegal_argument_exception", reason))
}

/// The `sltr` named `name` in the clauses of `q`.
fn named<'q>(q: &'q Query, name: &str) -> Option<&'q Sltr> {
    match q {
        Query::More(m) => match m.as_ref() {
            More::Sltr(s) if s.name.as_deref() == Some(name) => Some(s),
            _ => None,
        },
        Query::Bool(b) => b
            .must
            .iter()
            .chain(&b.filter)
            .chain(&b.should)
            .chain(&b.must_not)
            .find_map(|c| named(c, name)),
        _ => None,
    }
}

fn spec_sltr<'q>(spec: &LogSpec, source: &'q SearchSource) -> Result<&'q Sltr, ShardError> {
    if let Some(name) = &spec.named_query {
        return source
            .query
            .as_ref()
            .and_then(|q| named(q, name))
            // ALLOC: error path; one message.
            .ok_or_else(|| bad(format!("No query named [{name}] found")));
    }
    let at = spec.rescore_index.unwrap_or(0);
    match source.rescore.get(at).map(|r| &r.query) {
        Some(Query::More(m)) => match m.as_ref() {
            More::Sltr(s) => Ok(s),
            // ALLOC: error path; one message.
            _ => Err(bad(format!("The rescore [{at}] is not an [sltr] query"))),
        },
        // ALLOC: error path; one message.
        _ => Err(bad(format!("No rescore at [rescore_index] [{at}]"))),
    }
}

fn features_of(q: &Sltr) -> Result<(Vec<Feature>, String), ShardError> {
    let Some(models) = installed() else {
        return Err(bad(
            // ALLOC: error path; one message.
            "an [sltr] query needs serverless.ltr.enabled on this node".to_owned(),
        ));
    };
    let fail = |e: ResolveError| match e {
        ResolveError::NotFound(m) => ShardError {
            status: 404,
            failure: Failure::plain("resource_not_found_exception", m),
        },
        ResolveError::TooBig(m) => ShardError {
            status: 429,
            failure: Failure::plain("circuit_breaking_exception", m),
        },
        ResolveError::Failed(m) => ShardError {
            status: 500,
            failure: Failure::plain("ltr_store_failure", m),
        },
    };
    let store = q.store.as_deref();
    if let Some(set) = &q.featureset {
        let found = models.feature_set(store, set).map_err(fail)?;
        // ALLOC: one copy of the set's features, once a request.
        let copy = found
            .features
            .iter()
            .map(|f| Feature {
                name: f.name.clone(),
                params: f.params.clone(),
                template: f.template.clone(),
            })
            // ALLOC: one entry a feature, bounded by the set.
            .collect();
        // ALLOC: one name.
        return Ok((copy, set.clone()));
    }
    let model = models.resolve(store, &q.model).map_err(fail)?;
    // ALLOC: one copy of the model's features, once a request.
    let copy = model
        .features
        .iter()
        .map(|f| Feature {
            name: f.name.clone(),
            params: f.params.clone(),
            template: f.template.clone(),
        })
        // ALLOC: one entry a feature, bounded by the set.
        .collect();
    // ALLOC: one name.
    Ok((copy, q.model.clone()))
}

/// Each log's features and queries; none for a search with no `ltr_log`.
pub(crate) fn plan<S: BlobStore + 'static>(
    source: &SearchSource,
    cx: &QueryContext<'_>,
    shard: &OpenShard<S>,
    index: &str,
) -> Result<Plan, ShardError> {
    if source.ltr_log.is_empty() {
        // ALLOC: empty.
        return Ok(Plan { logs: Vec::new() });
    }
    // ALLOC: one entry a log, at most MAX_LOGS.
    let mut logs = Vec::with_capacity(source.ltr_log.len());
    for spec in &source.ltr_log {
        let q = spec_sltr(spec, source)?;
        let (features, what) = features_of(q)?;
        let queries = feature_queries(&features, (q, &what), cx, shard, index)?;
        logs.push(One {
            // ALLOC: one name a log.
            name: spec.name.clone(),
            names: features.iter().map(|f| f.name.clone()).collect(),
            queries,
            missing_as_zero: spec.missing_as_zero,
        });
    }
    Ok(Plan { logs })
}

impl Plan {
    /// Whether a feature query needs the points open.
    pub(crate) fn has_points(&self) -> bool {
        self.logs
            .iter()
            .any(|l| l.queries.iter().flatten().any(crate::search::has_points))
    }

    /// The text fields the feature queries score, whose norms they read.
    pub(crate) fn scored_fields(&self) -> Vec<String> {
        // ALLOC: one name a scored field, bounded by the feature queries.
        let mut out: Vec<String> = Vec::new();
        for q in self.logs.iter().flat_map(|l| l.queries.iter().flatten()) {
            for f in crate::search::scored_fields(q) {
                if !out.contains(&f) {
                    out.push(f);
                }
            }
        }
        out
    }

    /// Adds `_ltrlog` to the fields of `hits`, whose page it scores.
    pub(crate) fn attach<S: BlobStore + 'static>(
        &self,
        shard: &OpenShard<S>,
        segments: &[OpenSegment<'_>],
        hits: &mut crate::search::Hits,
    ) -> Result<(), ShardError> {
        if self.logs.is_empty() {
            return Ok(());
        }
        let logged = self.fill(shard, segments, &hits.hits)?;
        hits.fields.push(HitField {
            // ALLOC: one name, once a search.
            name: "_ltrlog".to_owned(),
            kind: ValueKind::LtrLog,
            times: 1,
            script: None,
            source: None,
            // ALLOC: one box a search.
            ltr: Some(std::sync::Arc::new(logged)),
        });
        Ok(())
    }

    /// Scores every feature of every log against `hits`.
    fn fill<S: BlobStore + 'static>(
        &self,
        shard: &OpenShard<S>,
        segments: &[OpenSegment<'_>],
        hits: &[Hit],
    ) -> Result<LtrLogged, ShardError> {
        let owned = shard
            .reader
            .field_norms_by_field(&self.scored_fields())
            .map_err(ShardError::engine)?;
        // ALLOC: one entry a segment.
        let norms: Vec<Option<&HashMap<String, FieldNorms<'_>>>> = owned.iter().map(Some).collect();
        let searcher = IndexSearcher::new(segments, &norms).map_err(ShardError::engine)?;
        let ctx = ValuesContext::new(&searcher);
        // ALLOC: one entry a hit of the page.
        let window: Vec<ShardScoreDoc> = hits
            .iter()
            .map(|h| ShardScoreDoc::new(h.doc, h.score.unwrap_or(0.0)))
            // ALLOC: one entry a feature, bounded by the set.
            .collect();
        // ALLOC: one entry a log, at most MAX_LOGS.
        let mut logs = Vec::with_capacity(self.logs.len());
        for log in &self.logs {
            let flat = vectors(&ctx, &window, &log.queries)?;
            let width = log.queries.len();
            // ALLOC: one row a hit of the page.
            let mut rows = HashMap::with_capacity(window.len());
            for (i, hit) in window.iter().enumerate() {
                let row = flat
                    .get(i.saturating_mul(width)..i.saturating_add(1).saturating_mul(width))
                    .unwrap_or_default()
                    .iter()
                    .map(|v| {
                        if v.is_nan() && log.missing_as_zero {
                            0.0
                        } else {
                            *v
                        }
                    })
                    // ALLOC: one value a feature.
                    .collect();
                rows.insert(hit.doc, row);
            }
            logs.push(LogRows {
                name: log.name.clone(),
                features: log.names.clone(),
                rows,
            });
        }
        Ok(LtrLogged { logs })
    }
}

/// Writes `"_ltrlog":[{"<log>":[{"name":..,"value":..},..]},..]` for the hit
/// with global document id `doc`: a feature with no value has its name only.
///
/// # Errors
///
/// An I/O error from `out`.
pub fn write(out: &mut dyn std::io::Write, logged: &LtrLogged, doc: i32) -> std::io::Result<()> {
    out.write_all(b"\"_ltrlog\":[")?;
    let mut first = true;
    for log in &logged.logs {
        let Some(row) = log.rows.get(&doc) else {
            continue;
        };
        out.write_all(if first { b"{" } else { b",{" })?;
        first = false;
        oss_rest::write_json_str(out, &log.name)?;
        out.write_all(b":[")?;
        for (i, (name, value)) in log.features.iter().zip(row).enumerate() {
            if i > 0 {
                out.write_all(b",")?;
            }
            out.write_all(b"{\"name\":")?;
            oss_rest::write_json_str(out, name)?;
            if !value.is_nan() {
                out.write_all(b",\"value\":")?;
                crate::response::write_java_float(out, *value)?;
            }
            out.write_all(b"}")?;
        }
        out.write_all(b"]}")?;
    }
    out.write_all(b"]")
}
