//! One search over an index's shards: ask each shard for its hits, merge
//! them, fetch the page from the shards that ranked it, reduce the metric
//! aggregations and write the response.
//!
//! The shards are a port ([`Shards`]): in one process they are the node's
//! readers; a later piece forwards to another node.

use std::fmt::Display;
use std::io::Write;

use oss_doc::mapping::json::write_str;
use oss_reader::aggs::metric::render::write_aggregations_with;
use oss_reader::aggs::partial::Partial;
use oss_reader::response::write_java_float;
use oss_reader::search::Hits;

use crate::fanout::{self, TaskError};
use crate::reduce::aggs::{AggsError, reduce_shards};
use crate::reduce::hits::{MergeError, Merger, Relation, ShardAnswer, ShardHit, SortSpec};
use crate::reduce::top_hits::{RenderError, fetch_page};

/// The shards of one search.
pub trait Shards: Sync {
    /// What a shard's query leaves for rendering its hits.
    type Query: Send + Sync;
    /// Why a shard's query or render failed.
    type Error: Send + Display;

    /// The index shard `shard` belongs to.
    fn index(&self, shard: usize) -> &str;

    /// Runs the search's query on `shard`, asking its top `from + size`.
    ///
    /// # Errors
    ///
    /// Whatever kept the shard from answering.
    fn query(&self, shard: usize) -> Result<Self::Query, Self::Error>;

    /// The hits `query` found.
    fn hits<'a>(&self, query: &'a Self::Query) -> &'a Hits;

    /// Writes the JSON object of each of `positions` among `query`'s hits,
    /// in the order asked.
    ///
    /// # Errors
    ///
    /// Whatever kept the shard from rendering.
    fn render(
        &self,
        shard: usize,
        query: &Self::Query,
        positions: &[usize],
    ) -> Result<Vec<Vec<u8>>, Self::Error>;
}

/// What a search asks of the merge and the response.
#[derive(Clone, Debug)]
pub struct Params {
    /// Hits skipped before the page.
    pub from: usize,
    /// Hits in the page.
    pub size: usize,
    /// The sort keys' directions; by score when `None`.
    pub sorts: Option<Vec<SortSpec>>,
    /// The total's ceiling (`track_total_hits`); `None` counts every one.
    pub track_total_hits_up_to: Option<i64>,
    /// Whether the response carries the total.
    pub show_total: bool,
    /// How many shards are asked at once.
    pub concurrency: usize,
    /// The `took` the response reports.
    pub took_ms: u64,
}

/// Why a search has no response.
#[derive(Debug, PartialEq)]
pub enum SearchFailure<E> {
    /// No shard answered; the first shard's error.
    AllFailed {
        /// The first shard's error.
        error: E,
    },
    /// The shards' hits cannot be merged.
    Merge(MergeError),
    /// The shards' aggregations cannot be reduced.
    Aggs(AggsError),
    /// A shard could not render the hits the page took from it.
    Fetch {
        /// The shard's number.
        shard: usize,
        /// Its error.
        reason: String,
    },
}

/// Runs the search over `shards` shards and writes its response.
///
/// # Errors
///
/// [`SearchFailure`]: every shard failed, the answers do not merge, or a
/// shard could not render its part of the page.
pub fn search<T: Shards>(
    t: &T,
    shards: usize,
    p: &Params,
) -> Result<Vec<u8>, SearchFailure<T::Error>> {
    // ALLOC: bounded by the shards, the page or the request's aggregations.
    let numbers: Vec<usize> = (0..shards).collect();
    let outcomes = fanout::run(p.concurrency, &numbers, |&shard| t.query(shard));
    // ALLOC: one slot a shard.
    let mut queries: Vec<Option<T::Query>> = Vec::with_capacity(shards);
    // ALLOC: one entry a failed shard.
    let mut failures: Vec<(usize, String)> = Vec::new();
    let mut first_error = None;
    for (shard, outcome) in outcomes.into_iter().enumerate() {
        match outcome {
            Ok(query) => queries.push(Some(query)),
            Err(failed) => {
                queries.push(None);
                let reason = match failed {
                    TaskError::Failed(error) => {
                        // ALLOC: bounded by the shards, the page or the request's aggregations.
                        let reason = error.to_string();
                        first_error.get_or_insert(error);
                        reason
                    }
                    // ALLOC: bounded by the shards, the page or the request's aggregations.
                    TaskError::Panicked => "the shard's query panicked".to_owned(),
                };
                failures.push((shard, reason));
            }
        }
    }
    let answered = queries.iter().flatten().count();
    if answered == 0 && shards > 0 {
        return match first_error {
            Some(error) => Err(SearchFailure::AllFailed { error }),
            None => Err(SearchFailure::Fetch {
                shard: 0,
                // ALLOC: bounded by the shards, the page or the request's aggregations.
                reason: "the shard's query panicked".to_owned(),
            }),
        };
    }
    let mut merger: Merger<(usize, usize)> = Merger::new(p.from, p.size, p.sorts.clone());
    for (shard, query) in queries.iter().enumerate() {
        let Some(query) = query else { continue };
        let found = t.hits(query);
        let hits = found
            .hits
            .iter()
            .enumerate()
            .map(|(at, hit)| ShardHit {
                score: hit.score,
                sort: hit.sort.clone(),
                payload: (shard, at),
            })
            // ALLOC: one entry a hit the shard kept, at most from + size.
            .collect();
        merger
            .absorb(ShardAnswer {
                total: found.total,
                exact: found.exact,
                max_score: found.max_score,
                hits,
                timed_out: false,
                terminated_early: None,
            })
            .map_err(SearchFailure::Merge)?;
    }
    let merged = merger
        .finish(p.track_total_hits_up_to)
        .map_err(SearchFailure::Merge)?;
    // ALLOC: bounded by the shards, the page or the request's aggregations.
    let page: Vec<(usize, usize)> = merged.page.iter().map(|h| h.payload).collect();
    let rendered = fetch_page(&page, p.concurrency, |shard, positions| {
        match queries.get(shard).and_then(Option::as_ref) {
            Some(query) => t.render(shard, query, positions),
            // ALLOC: bounded by the shards, the page or the request's aggregations.
            None => Ok(Vec::new()),
        }
    })
    .map_err(|e| fetch_failure(&e))?;
    let parts = reduce_aggregations(t, &queries)?;
    // ALLOC: the response, bounded by the page's rendered hits.
    let mut out = Vec::new();
    // Writing to a vector cannot fail.
    let _ = write_envelope(
        &mut out, p, &merged, &rendered, &failures, shards, answered, t, &parts,
    );
    Ok(out)
}

/// The failure a render error is.
fn fetch_failure<E: Display, T>(e: &RenderError<E>) -> SearchFailure<T> {
    match e {
        RenderError::Failed { shard, error } => SearchFailure::Fetch {
            shard: *shard,
            // ALLOC: bounded by the shards, the page or the request's aggregations.
            reason: error.to_string(),
        },
        RenderError::Panicked { shard } => SearchFailure::Fetch {
            shard: *shard,
            // ALLOC: bounded by the shards, the page or the request's aggregations.
            reason: "the shard's render panicked".to_owned(),
        },
        RenderError::WrongCount { shard, asked, got } => SearchFailure::Fetch {
            shard: *shard,
            // ALLOC: bounded by the shards, the page or the request's aggregations.
            reason: format!("asked {asked} hits and got {got}"),
        },
    }
}

/// The answering shards' aggregations reduced, as the first shard's
/// partials hold their names and formats.
fn reduce_aggregations<T: Shards>(
    t: &T,
    queries: &[Option<T::Query>],
) -> Result<Vec<Partial>, SearchFailure<T::Error>> {
    // ALLOC: one entry an answering shard.
    let answers: Vec<&Hits> = queries.iter().flatten().map(|q| t.hits(q)).collect();
    let values: Vec<Vec<_>> = answers
        .iter()
        .map(|h| {
            h.aggregations
                .iter()
                // ALLOC: one entry an aggregation of the request.
                .map(|a| (a.name.clone(), a.value.clone()))
                .collect()
        })
        // ALLOC: bounded by the shards, the page or the request's aggregations.
        .collect();
    let reduced = reduce_shards(&values).map_err(SearchFailure::Aggs)?;
    let first = answers.first().map(|h| &h.aggregations);
    Ok(reduced
        .into_iter()
        .map(|(name, value)| Partial {
            format: first
                .and_then(|f| f.iter().find(|a| a.name == name))
                .map(|a| a.format)
                .unwrap_or(oss_reader::aggs::partial::Format::Raw),
            name,
            value,
        })
        // ALLOC: one entry an aggregation of the request.
        .collect())
}

/// Writes the response body.
#[allow(clippy::too_many_arguments)]
fn write_envelope<T: Shards>(
    out: &mut Vec<u8>,
    p: &Params,
    merged: &crate::reduce::hits::Merged<(usize, usize)>,
    rendered: &[Vec<u8>],
    failures: &[(usize, String)],
    shards: usize,
    answered: usize,
    t: &T,
    parts: &[Partial],
) -> std::io::Result<()> {
    write!(
        out,
        "{{\"took\":{},\"timed_out\":false,\"_shards\":{{\"total\":{shards},\"successful\":{answered},\"skipped\":0,\"failed\":{}",
        p.took_ms,
        failures.len()
    )?;
    if !failures.is_empty() {
        out.write_all(b",\"failures\":[")?;
        for (i, (shard, reason)) in failures.iter().enumerate() {
            if i > 0 {
                out.write_all(b",")?;
            }
            // ALLOC: one escaped string a failed shard.
            let mut text = String::new();
            write_str(&mut text, t.index(*shard));
            let mut why = String::new();
            write_str(&mut why, reason);
            write!(
                out,
                "{{\"shard\":{shard},\"index\":{text},\"reason\":{{\"type\":\"exception\",\"reason\":{why}}}}}"
            )?;
        }
        out.write_all(b"]")?;
    }
    out.write_all(b"},\"hits\":{")?;
    if p.show_total {
        let relation = match merged.relation {
            Relation::Equal => "eq",
            Relation::AtLeast => "gte",
        };
        write!(
            out,
            "\"total\":{{\"value\":{},\"relation\":\"{relation}\"}},",
            merged.total
        )?;
    }
    out.write_all(b"\"max_score\":")?;
    match merged.max_score {
        Some(score) => write_java_float(out, score)?,
        None => out.write_all(b"null")?,
    }
    out.write_all(b",\"hits\":[")?;
    for (i, hit) in rendered.iter().enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        out.write_all(hit)?;
    }
    out.write_all(b"]}")?;
    write_aggregations_with(out, parts, &mut |_, _| Ok(()))?;
    out.write_all(b"}")
}
