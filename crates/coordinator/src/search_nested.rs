//! The `top_hits` under buckets of a search's aggregations: tagged with their
//! shard before the reduce, and after it merged across the shards that hold
//! the bucket and written from the shards that ranked each page.

use std::convert::Infallible;

use oss_reader::aggs::bucket::walk::walk_subs_mut;
use oss_reader::aggs::metric::top_hits::TopHitsResult;
use oss_reader::aggs::partial::Value;

use crate::reduce::aggs::AggsError;
use crate::reduce::top_hits::{RenderError, reduce_top_hits, render_page};
use crate::search::{SearchFailure, TopHitsWindow};

/// Writes the hits at positions of a `top_hits` result of a shard.
pub type Render<'a, E> =
    dyn Fn(usize, &TopHitsResult, &[usize]) -> Result<Vec<Vec<u8>>, E> + Sync + 'a;

/// Marks each `top_hits` under a bucket of `value` as `shard`'s.
pub fn tag_origins(value: &mut Value, shard: usize) {
    let _ = walk_subs_mut::<Infallible>(value, &mut |_, v| {
        if let Value::TopHits(top) = v {
            top.origin = shard;
        }
        Ok(())
    });
}

/// Replaces each `top_hits` under a bucket of `value` by its merged hits,
/// written: `window` gives a `top_hits`'s `from`, `size` and sort by name,
/// and `render` writes the hits at positions of a name's result on a shard.
///
/// # Errors
///
/// [`SearchFailure`]: a `top_hits` the request does not describe, hits that
/// do not merge, or a shard that could not write its hits.
pub fn finish<E: std::fmt::Display + Send>(
    value: &mut Value,
    concurrency: usize,
    window: &dyn Fn(&str) -> Option<TopHitsWindow>,
    render: &Render<'_, E>,
) -> Result<(), SearchFailure<E>> {
    walk_subs_mut(value, &mut |name, v| {
        let parts = match v {
            Value::TopHitsParts(parts) => std::mem::take(parts),
            // A bucket no shard holds: the empty result a reduce fills in.
            // ALLOC: one result, once a bucket.
            Value::TopHits(top) => vec![(**top).clone()],
            _ => return Ok(()),
        };
        let Some(w) = window(name) else {
            return Err(SearchFailure::Aggs(AggsError::TopHits {
                // ALLOC: error path; one name.
                name: name.to_owned(),
            }));
        };
        let merged =
            reduce_top_hits(&parts, w.from, w.size, w.sorts).map_err(SearchFailure::Merge)?;
        let body = render_page(&merged, concurrency, |at, positions| {
            let Some(part) = parts.get(at) else {
                // ALLOC: empty.
                return Ok(Vec::new());
            };
            if positions.is_empty() {
                // ALLOC: empty.
                return Ok(Vec::new());
            }
            render(part.origin, part, positions)
        })
        .map_err(|e| match e {
            RenderError::Failed { shard, error } => SearchFailure::Fetch {
                shard,
                // ALLOC: error path; one message.
                reason: error.to_string(),
            },
            RenderError::Panicked { shard } => SearchFailure::Fetch {
                shard,
                // ALLOC: error path; one message.
                reason: "the shard's render panicked".to_owned(),
            },
            RenderError::WrongCount { shard, asked, got } => SearchFailure::Fetch {
                shard,
                // ALLOC: error path; one message.
                reason: format!("asked for {asked} hits and was given {got}"),
            },
        })?;
        *v = Value::Rendered(body);
        Ok(())
    })
}
