//! One search over an index's shards: ask each shard for its hits, merge
//! them, fetch the page from the shards that ranked it, reduce the metric
//! aggregations and write the response.
//!
//! The shards are a port ([`Shards`]): in one process they are the node's
//! readers; a later piece forwards to another node.

use std::fmt::Display;
use std::io::Write;

use oss_doc::mapping::json::write_str;
use oss_reader::aggs::metric::render::write_aggregations_piped;
use oss_reader::aggs::partial::Partial;
use oss_reader::response::write_java_float;
use oss_reader::search::Hits;

use crate::fanout::{self, TaskError};
use crate::reduce::aggs::{AggsError, reduce_shards};
use crate::reduce::hits::{MergeError, Merger, Relation, ShardAnswer, ShardHit, SortSpec};
use crate::reduce::top_hits::{RenderError, fetch_page, reduce_top_hits, render_page};
use crate::search_nested;
use oss_reader::aggs::metric::top_hits::TopHitsResult;
use oss_reader::aggs::partial::Value;

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

    /// The window and sort of the request's top-level `top_hits` named
    /// `name`: `from`, `size` and the sort's directions (by score when
    /// `None`). `None` when the request has no such aggregation, which a
    /// merge of its shards' hits then refuses.
    fn top_hits_window(&self, name: &str) -> Option<TopHitsWindow> {
        let _ = name;
        None
    }

    /// As [`Self::render`], for the hits of a `top_hits` `result` that
    /// `shard` produced (the aggregation's or one bucket's), at `positions`
    /// counted over its `before` then its `hits`.
    ///
    /// # Errors
    ///
    /// Whatever kept the shard from rendering.
    fn render_top_hits(
        &self,
        shard: usize,
        query: &Self::Query,
        result: &TopHitsResult,
        positions: &[usize],
    ) -> Result<Vec<Vec<u8>>, Self::Error>;
}

/// What a `top_hits` asks of the merge of its shards' hits.
#[derive(Clone, Debug)]
pub struct TopHitsWindow {
    /// Hits skipped before the page.
    pub from: usize,
    /// Hits in the page.
    pub size: usize,
    /// The sort keys' directions; by score when `None`.
    pub sorts: Option<Vec<SortSpec>>,
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
    /// Whether the total is written as a bare number (`rest_total_hits_as_int`).
    pub total_as_int: bool,
    /// Whether each aggregation's key carries its type (`typed_keys`).
    pub typed_keys: bool,
    /// The sibling pipeline aggregations to compute over the reduced aggregations.
    pub pipelines: Vec<oss_reader::aggs::pipeline::PipelineAgg>,
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
    oss_reader::hits_total::scoped(p.total_as_int, || search_scoped(t, shards, p))
}

/// [`search`] inside the scope of its totals.
fn search_scoped<T: Shards>(
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
                terminated_early: found.terminated_early,
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
    let (parts, top_bodies) = reduce_aggregations(t, &queries, p)?;
    // ALLOC: the response, bounded by the page's rendered hits.
    let mut out = Vec::new();
    // Writing to a vector cannot fail.
    let _ = write_envelope(
        &mut out,
        p,
        &merged,
        &rendered,
        &failures,
        shards,
        answered,
        t,
        &parts,
        &top_bodies,
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

/// A `top_hits` reduced: its name and its written `"hits":{..}` body.
type TopBody = (String, Vec<u8>);

/// The reduced aggregations and the `top_hits` bodies written with them.
type Reduced = (Vec<Partial>, Vec<TopBody>);

/// The answering shards' aggregations reduced, as the first shard's
/// partials hold their names and formats, and the body of each top-level
/// `top_hits` merged across the shards and fetched from the ones that
/// ranked its page.
fn reduce_aggregations<T: Shards>(
    t: &T,
    queries: &[Option<T::Query>],
    p: &Params,
) -> Result<Reduced, SearchFailure<T::Error>> {
    let answering: Vec<(usize, &T::Query)> = queries
        .iter()
        .enumerate()
        .filter_map(|(shard, q)| q.as_ref().map(|q| (shard, q)))
        // ALLOC: one entry an answering shard.
        .collect();
    // ALLOC: bounded by the shards, the page or the request's aggregations.
    let answers: Vec<&Hits> = answering.iter().map(|(_, q)| t.hits(q)).collect();
    // ALLOC: bounded by the shards, the page or the request's aggregations.
    let mut tops: Vec<TopBody> = Vec::new();
    let mut values: Vec<Vec<_>> = answers
        .iter()
        .map(|h| {
            h.aggregations
                .iter()
                .filter(|a| {
                    !matches!(a.value, Value::TopHits(_)) || t.top_hits_window(&a.name).is_none()
                })
                // ALLOC: one entry an aggregation of the request.
                .map(|a| (a.name.clone(), a.value.clone()))
                .collect()
        })
        // ALLOC: bounded by the shards, the page or the request's aggregations.
        .collect();
    for (list, (shard, _)) in values.iter_mut().zip(&answering) {
        for (_, value) in list {
            search_nested::tag_origins(value, *shard);
        }
    }
    let mut reduced = reduce_shards(&values).map_err(SearchFailure::Aggs)?;
    for (_, value) in &mut reduced {
        search_nested::finish(
            value,
            p.concurrency,
            &|name| t.top_hits_window(name),
            &|shard, result, positions| match queries.get(shard).and_then(Option::as_ref) {
                Some(query) => t.render_top_hits(shard, query, result, positions),
                // ALLOC: empty.
                None => Ok(Vec::new()),
            },
        )?;
    }
    let first = answers.first().map(|h| &h.aggregations);
    // ALLOC: bounded by the shards, the page or the request's aggregations.
    let mut names: Vec<&str> = Vec::new();
    for h in &answers {
        for a in &h.aggregations {
            if matches!(a.value, Value::TopHits(_))
                && t.top_hits_window(&a.name).is_some()
                && !names.contains(&a.name.as_str())
            {
                names.push(&a.name);
            }
        }
    }
    for name in names {
        let Some(window) = t.top_hits_window(name) else {
            continue;
        };
        // The answering shards that hold this aggregation, with their results.
        // ALLOC: bounded by the shards, the page or the request's aggregations.
        let mut owners: Vec<usize> = Vec::new();
        // ALLOC: bounded by the shards, the page or the request's aggregations.
        let mut results = Vec::new();
        for ((shard, _), h) in answering.iter().zip(&answers) {
            if let Some(Value::TopHits(top)) = h
                .aggregations
                .iter()
                .find(|a| a.name == name)
                .map(|a| &a.value)
            {
                owners.push(*shard);
                results.push((**top).clone());
            }
        }
        let merged = reduce_top_hits(&results, window.from, window.size, window.sorts)
            .map_err(SearchFailure::Merge)?;
        let body = render_page(&merged, p.concurrency, |at, positions| {
            let shard = owners.get(at).copied().unwrap_or(usize::MAX);
            match queries.get(shard).and_then(Option::as_ref) {
                Some(query) => match results.get(at) {
                    Some(result) => t.render_top_hits(shard, query, result, positions),
                    // ALLOC: empty.
                    None => Ok(Vec::new()),
                },
                // ALLOC: error path only.
                None => Ok(Vec::new()),
            }
        })
        .map_err(|e| fetch_failure(&e))?;
        // The writer takes the body by name; the value only marks the kind.
        // ALLOC: bounded by the shards, the page or the request's aggregations.
        reduced.push((
            name.to_owned(),
            Value::TopHits(Box::new(
                oss_reader::aggs::metric::top_hits::TopHitsResult::empty(),
            )),
        ));
        // ALLOC: bounded by the shards, the page or the request's aggregations.
        tops.push((name.to_owned(), body));
    }
    let parts = reduced
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
        .collect();
    Ok((parts, tops))
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
    top_bodies: &[(String, Vec<u8>)],
) -> std::io::Result<()> {
    write!(out, "{{\"took\":{},\"timed_out\":false", p.took_ms,)?;
    if let Some(early) = merged.terminated_early {
        write!(out, ",\"terminated_early\":{early}")?;
    }
    write!(
        out,
        ",\"_shards\":{{\"total\":{shards},\"successful\":{answered},\"skipped\":0,\"failed\":{}",
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
    if p.show_total && p.total_as_int {
        write!(out, "\"total\":{},", merged.total)?;
    } else if p.show_total {
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
    write_aggregations_piped(
        out,
        parts,
        &p.pipelines,
        p.typed_keys,
        &mut |out, name, _| match top_bodies.iter().find(|(n, _)| n == name) {
            Some((_, body)) => out.write_all(body),
            None => Ok(()),
        },
    )?;
    out.write_all(b"}")
}
