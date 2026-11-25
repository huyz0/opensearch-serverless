//! The search half of a by-query walk, answered by this node's own
//! [`SearchService`]: a query sorted by `_seq_no`, each hit read back as its
//! id, sequence number and primary term.

use oss_blob::BlobStore;
use oss_coordinator::docs::by_query::Hit;
use oss_doc::mapping::json::{Json, parse};
use oss_reader::dsl::{SearchSource, parse_chunks};
use oss_rest::{Exception, RestError};

use super::ShardMatches;
use crate::search::SearchService;

/// [`ShardMatches`] over a [`SearchService`].
pub struct SearchMatches<S: BlobStore + 'static> {
    search: std::sync::Arc<SearchService<S>>,
}

impl<S: BlobStore + 'static> SearchMatches<S> {
    /// Matches found by `search`.
    pub fn new(search: std::sync::Arc<SearchService<S>>) -> Self {
        Self { search }
    }

    /// Runs `body` on one shard and reads each hit back.
    fn run(&self, index: &str, shard: u32, body: &str) -> Result<Vec<Hit>, RestError> {
        let source = parse_chunks([body.as_bytes()])
            .and_then(SearchSource::refuse_pipeline)
            .map_err(|e| {
                let f = e.into_failure();
                RestError::exception(400, Exception::new(f.kind, f.reason))
            })?;
        let found = self
            .search
            .query_shard(index, shard, &source)
            .map_err(|e| {
                RestError::exception(e.status, Exception::new(e.failure.kind, e.failure.reason))
            })?;
        let positions: Vec<usize> = (0..found.hits.hits.len()).collect();
        if positions.is_empty() {
            return Ok(Vec::new());
        }
        let rendered = found
            .render("", &positions, &mut |_| Ok(()))
            .map_err(internal)?;
        // ALLOC: one hit a rendered hit, at most the page size.
        let mut out = Vec::with_capacity(rendered.len());
        for bytes in &rendered {
            out.push(read_hit(bytes)?);
        }
        Ok(out)
    }
}

fn internal(e: impl std::fmt::Display) -> RestError {
    RestError::exception(
        500,
        // ALLOC: error path; one message.
        Exception::new("exception", format!("reading a matching document: {e}")),
    )
}

/// A rendered hit's `_id`, `_seq_no` and `_primary_term`.
fn read_hit(bytes: &[u8]) -> Result<Hit, RestError> {
    let hit = parse(bytes).map_err(|e| internal(e.reason))?;
    let long = |name: &str| -> Result<i64, RestError> {
        match hit
            .get("fields")
            .and_then(|f| f.get(name))
            .and_then(|v| match v {
                Json::Arr(a) => a.first(),
                _ => None,
            }) {
            Some(Json::Num(n)) => n.parse().map_err(internal),
            _ => Err(internal(format!("a hit without [{name}]"))),
        }
    };
    let Some(Json::Str(id)) = hit.get("_id") else {
        return Err(internal("a hit without an _id"));
    };
    Ok(Hit {
        // ALLOC: one id a hit.
        id: id.clone(),
        seq_no: long("_seq_no")?,
        primary_term: long("_primary_term")?,
    })
}

fn body(query: &str, order: &str, size: usize, after: Option<i64>) -> String {
    let after = after.map_or_else(String::new, |a| format!(",\"search_after\":[{a}]"));
    format!(
        "{{\"query\":{query},\"size\":{size},\"sort\":[{{\"_seq_no\":\"{order}\"}}]{after},\
         \"_source\":false,\"track_total_hits\":false,\
         \"docvalue_fields\":[\"_seq_no\",\"_primary_term\"]}}"
    )
}

impl<S: BlobStore + 'static> ShardMatches for SearchMatches<S> {
    fn newest(&self, index: &str, shard: u32, query: &str) -> Result<Option<i64>, RestError> {
        Ok(self
            .run(index, shard, &body(query, "desc", 1, None))?
            .first()
            .map(|h| h.seq_no))
    }

    fn page(
        &self,
        index: &str,
        shard: u32,
        query: &str,
        up_to: i64,
        after: Option<i64>,
        size: usize,
    ) -> Result<Vec<Hit>, RestError> {
        let mut hits = self.run(index, shard, &body(query, "asc", size, after))?;
        hits.retain(|h| h.seq_no <= up_to);
        Ok(hits)
    }
}
