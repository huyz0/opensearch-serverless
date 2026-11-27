//! A search over several indexes: a list of names, or an alias of several.
//!
//! Each name is an index or an alias, read as one is for a single index. The
//! shards of every index they resolve to are queried and merged by the
//! coordinator as one search; nothing is listed, a name is a literal name.

use std::time::Instant;

use oss_blob::BlobStore;
use oss_rest::{Request, Response, RestError};

#[cfg(feature = "coordinator")]
use super::coordinate::Visit;
use super::{Answer, SearchService, send, uri_query};

/// The most indexes one search may name or resolve to.
pub(super) const MAX_INDEXES: usize = 64;

/// The most shards, over all its indexes, one search may visit.
pub(super) const MAX_SHARDS: usize = 1_024;

fn too_many(what: &str, limit: usize) -> RestError {
    // ALLOC: error path; one message.
    RestError::illegal_argument(format!(
        "a search over several indexes may visit at most {limit} {what}"
    ))
}

/// What a request's `index` path parameter resolves to.
pub(super) enum Targets {
    /// One index, now the path parameter (or none named).
    One,
    /// Several indexes, in order, each once.
    Many(Vec<String>),
}

impl<S: BlobStore + 'static> SearchService<S> {
    /// Resolves the `index` path parameter: a single index is written back
    /// to the parameter, and several are returned.
    pub(super) fn resolve_targets(&self, req: &mut Request<'_>) -> Result<Targets, RestError> {
        let Some(path) = req.path_param("index") else {
            return Ok(Targets::One);
        };
        if path.contains(['*', '?']) || path == "_all" {
            return Ok(Targets::One);
        }
        // ALLOC: one name per entry of the path, bounded by its length.
        let mut resolved: Vec<String> = Vec::new();
        for name in path.split(',').filter(|n| !n.is_empty()) {
            if resolved.len() >= MAX_INDEXES {
                return Err(too_many("indexes", MAX_INDEXES));
            }
            let members = self.alias_members(name)?;
            // ALLOC: one name per member or entry.
            let names = members.unwrap_or_else(|| vec![name.to_owned()]);
            for index in names {
                if !resolved.contains(&index) {
                    if resolved.len() >= MAX_INDEXES {
                        return Err(too_many("indexes", MAX_INDEXES));
                    }
                    resolved.push(index);
                }
            }
        }
        match resolved.len() {
            0 => Ok(Targets::One),
            1 => {
                if let Some(index) = resolved.pop() {
                    req.rename_path_param("index", index);
                }
                Ok(Targets::One)
            }
            _ => Ok(Targets::Many(resolved)),
        }
    }

    /// Answers a `_search` over `indexes` as one response.
    pub(super) fn handle_many(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
        indexes: &[String],
    ) -> Result<(), RestError> {
        let started = Instant::now();
        let mut source = match uri_query::read_search_source(req) {
            Ok(source) => source,
            Err(Answer::Rest(e)) => return Err(e),
            Err(Answer::Search(e)) => return send(resp, &e),
        };
        // No index's default pipeline runs over several.
        match self.run_pipeline(req, &mut source, None) {
            Ok(()) => {}
            Err(Answer::Rest(e)) => return Err(e),
            Err(Answer::Search(e)) => return send(resp, &e),
        }
        if source.pit.is_some() {
            return Err(RestError::illegal_argument(
                "a search of a point in time names no index: the view holds it",
            ));
        }
        if source.collapse.is_some() {
            return Err(RestError::illegal_argument(
                "[collapse] over several indexes is not supported by this node yet",
            ));
        }
        #[cfg(feature = "coordinator")]
        {
            let routing = req.param("routing");
            // ALLOC: one entry per shard of every index searched.
            let mut members: Vec<(String, u32)> = Vec::new();
            for index in indexes {
                let (count, only) = match self.shard_plan(index, routing.as_deref()) {
                    Ok(plan) => plan,
                    Err(Answer::Rest(e)) => return Err(e),
                    Err(Answer::Search(e)) => return send(resp, &e),
                };
                if members
                    .len()
                    .saturating_add(only.as_ref().map_or(count, Vec::len))
                    > MAX_SHARDS
                {
                    return Err(too_many("shards", MAX_SHARDS));
                }
                match only {
                    // ALLOC: one name per shard.
                    Some(only) => members.extend(only.into_iter().map(|n| (index.clone(), n))),
                    None => members.extend((0..count).filter_map(|n| {
                        // ALLOC: one name per shard.
                        u32::try_from(n).ok().map(|n| (index.clone(), n))
                    })),
                }
            }
            let visit = Visit {
                shards: members.len(),
                only: None,
                members: Some(&members),
            };
            // The coordinator names each shard's index from `members`; this
            // one only labels the call.
            let first = indexes.first().map_or("", String::as_str);
            self.handle_sharded(resp, first, visit, &source, started)
        }
        #[cfg(not(feature = "coordinator"))]
        {
            let _ = (resp, started, indexes);
            Err(RestError::illegal_argument(
                "a search over several indexes needs the coordinator role",
            ))
        }
    }
}
