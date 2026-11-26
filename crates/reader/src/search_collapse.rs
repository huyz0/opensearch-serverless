//! `collapse` on one shard: `CollapsingTopDocsCollector` (a first-pass
//! grouping collector) at OpenSearch 3.10.0. The best hit of each distinct
//! value of a keyword or number field answers, in the order of those hits.
//!
//! The hits of a search are collected in its order over a window, grouped as
//! they come, and the window grows while groups are missing and matches
//! remain, to a cap: the first groups of the best `window` hits are the
//! best groups, since a group's rank is its first hit's.

use std::collections::{HashMap, HashSet};
use std::io;

use lucene_search::BooleanQuery;
use lucene_search::field_norms::FieldNorms;
use lucene_search::multi_segment::OpenSegment;
use lucene_search::top_field::SortField;
use oss_blob::BlobStore;

use crate::OpenShard;
use crate::dsl::SearchSource;
use crate::failure::Failure;
use crate::fetch::docvalue_fields::{self, Value, ValueKind};
use crate::mapping::{FieldType, Mapping};
use crate::search::{Hit, ShardError};
use crate::search_sort::KeyKind;
use crate::search_top::{Top, TopOptions, collect_top};

/// The most candidate hits a collapse collects before it refuses the search.
pub const MAX_CANDIDATES: usize = 100_000;

/// A search's collapse field and how it reads.
pub(crate) struct Plan {
    field: String,
    kind: ValueKind,
}

fn bad_request(reason: String) -> ShardError {
    ShardError::bad_request(Failure::plain("illegal_argument_exception", reason))
}

/// `CollapseBuilder.build` and `SearchService.parseSource`'s collapse checks.
pub(crate) fn plan(source: &SearchSource, mapping: &Mapping) -> Result<Option<Plan>, ShardError> {
    let Some(collapse) = &source.collapse else {
        return Ok(None);
    };
    let name = &collapse.field;
    if !source.search_after.is_empty() {
        return Err(bad_request(
            // ALLOC: error path; one message.
            "[collapse] with [search_after] is not supported by this node yet".to_owned(),
        ));
    }
    if !source.rescore.is_empty() {
        return Err(bad_request(
            // ALLOC: error path; one message.
            "cannot use `collapse` in conjunction with `rescore`".to_owned(),
        ));
    }
    let Some(field) = mapping.get(name) else {
        // ALLOC: error path; one message.
        return Err(bad_request(format!(
            "no mapping found for `{name}` in order to collapse on"
        )));
    };
    let kind = match field.ty {
        FieldType::Keyword => ValueKind::Term,
        FieldType::Int { .. } | FieldType::Long => ValueKind::Long,
        _ => {
            // ALLOC: error path; one message.
            return Err(bad_request(format!(
                // ALLOC: error path; one message.
                "unknown type for collapse field `{name}`, only keywords and numbers are accepted"
            )));
        }
    };
    if !field.doc_values {
        // ALLOC: error path; one message.
        return Err(bad_request(format!(
            // ALLOC: error path; one message.
            "cannot collapse on field `{name}` without `doc_values`"
        )));
    }
    Ok(Some(Plan {
        // ALLOC: one name a collapse.
        field: name.clone(),
        kind,
    }))
}

/// The window after `window`, none past the cap.
pub(crate) fn grow(window: usize, cap: usize) -> Option<usize> {
    (window < cap).then(|| window.saturating_mul(4).min(cap))
}

impl Plan {
    /// The first hit of each value among `hits`, in order: `None` for a
    /// document without a value is one group.
    fn groups(
        &self,
        shard_segments: &[&lucene_search::directory_reader::SegmentReader],
        hits: &[Hit],
    ) -> io::Result<Vec<Hit>> {
        let mut seen: HashSet<Option<Vec<u8>>> = HashSet::new();
        // ALLOC: one hit a group, at most the window.
        let mut out = Vec::new();
        for hit in hits {
            let Some(segment) = shard_segments.iter().rev().find(|r| r.doc_base <= hit.doc) else {
                return Err(io::Error::other("a hit outside every segment"));
            };
            let doc = hit.doc.saturating_sub(segment.doc_base);
            let values = match self.kind {
                ValueKind::Term => docvalue_fields::terms(segment, &self.field, doc)?,
                _ => docvalue_fields::numbers(segment, &self.field, doc)?,
            };
            let key = match values.as_slice() {
                [] => None,
                // ALLOC: one key a group, at most the window.
                [Value::Long(n)] => Some(n.to_be_bytes().to_vec()),
                [Value::Term(t)] => Some(t.clone()),
                _ => {
                    // ALLOC: error path; one message.
                    return Err(io::Error::other(format!(
                        "collapse on the multi-valued field [{}] is not supported",
                        self.field
                    )));
                }
            };
            if seen.insert(key) {
                out.push(hit.clone());
            }
        }
        Ok(out)
    }

    /// The top `window` groups of `query`, less the first `from`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn collect<S: BlobStore + 'static>(
        &self,
        segments: &[OpenSegment<'_>],
        shard: &OpenShard<S>,
        norms: &[Option<&HashMap<String, FieldNorms<'_>>>],
        query: &BooleanQuery,
        sort: Option<&(Vec<SortField>, Vec<KeyKind>)>,
        (window, from, limit): (usize, usize, u64),
        options: TopOptions,
    ) -> Result<Top, ShardError> {
        // ALLOC: one reader a segment.
        let readers: Vec<&_> = shard.reader.segment_readers().iter().collect();
        let mut candidates = window.saturating_mul(4).clamp(1, MAX_CANDIDATES);
        loop {
            let mut top = collect_top(
                segments, shard, norms, query, sort, candidates, 0, limit, None, options,
            )?;
            let collected = top.hits.len();
            let groups = self
                .groups(&readers, &top.hits)
                // ALLOC: error path; one message.
                .map_err(|e| bad_request(e.to_string()))?;
            if groups.len() >= window || collected < candidates {
                top.hits = groups
                    .into_iter()
                    .skip(from)
                    .take(window.saturating_sub(from))
                    // ALLOC: one hit a group, at most the window.
                    .collect();
                return Ok(top);
            }
            candidates = grow(candidates, MAX_CANDIDATES).ok_or_else(|| {
                // ALLOC: error path; one message.
                bad_request(format!(
                    "collapse needs more than {MAX_CANDIDATES} candidate hits to find {window} groups"
                ))
            })?;
        }
    }
}

/// The top hits of a search, as its collapse and rescore ask: the groups'
/// first hits, the rescored window, or the plain top documents.
#[allow(clippy::type_complexity)]
pub(crate) fn top<S: BlobStore + 'static>(
    (collapse, rescore): (Option<&Plan>, Option<&crate::search_rescore::Plan>),
    (segments, shard, norms): (
        &[OpenSegment<'_>],
        &OpenShard<S>,
        &[Option<&HashMap<String, FieldNorms<'_>>>],
    ),
    (query, sort): (&BooleanQuery, Option<&(Vec<SortField>, Vec<KeyKind>)>),
    (window, from, limit, after): (
        usize,
        usize,
        u64,
        Option<&lucene_search::top_field::FieldDoc>,
    ),
    options: TopOptions,
) -> Result<Top, ShardError> {
    if let Some(plan) = collapse {
        // ALLOC: the groups' hits, at most the window.
        return plan.collect(
            segments,
            shard,
            norms,
            query,
            sort,
            (window, from, limit),
            options,
        );
    }
    let (window, from) = rescore.map_or((window, from), |r| r.collection(window));
    collect_top(
        segments, shard, norms, query, sort, window, from, limit, after, options,
    )
}

#[cfg(test)]
#[path = "search_collapse_tests.rs"]
mod tests;
