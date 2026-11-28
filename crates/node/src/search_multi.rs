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
#[cfg(feature = "coordinator")]
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
    /// None: every name was unavailable and `ignore_unavailable` is set.
    Nothing,
}

/// What a search answers when every index it named is unavailable and
/// `ignore_unavailable` is set.
pub(super) const NO_HITS: &[u8] = br#"{"took":0,"timed_out":false,"_shards":{"total":0,"successful":0,"skipped":0,"failed":0},"hits":{"total":{"value":0,"relation":"eq"},"max_score":null,"hits":[]}}"#;

/// [`NO_HITS`] under `rest_total_hits_as_int`.
const NO_HITS_AS_INT: &[u8] = br#"{"took":0,"timed_out":false,"_shards":{"total":0,"successful":0,"skipped":0,"failed":0},"hits":{"total":0,"max_score":null,"hits":[]}}"#;

impl<S: BlobStore + 'static> SearchService<S> {
    /// Resolves the `index` path parameter: a single index is written back
    /// to the parameter, and several are returned.
    pub(super) fn resolve_targets(&self, req: &mut Request<'_>) -> Result<Targets, RestError> {
        // Patterns are refused, so no wildcard option has anything to change but the one that names
        // what a pattern would expand to.
        if let Some(kind) = req.param("expand_wildcards").as_deref()
            && kind != "open"
        {
            return Err(RestError::illegal_argument(
                "an [expand_wildcards] other than open is not supported by this node yet",
            ));
        }
        let Some(path) = req.path_param("index") else {
            return Ok(Targets::One);
        };
        if path.contains(['*', '?']) || path == "_all" {
            return Ok(Targets::One);
        }
        let mut resolved = self.expand_names(path.split(','))?;
        if ignore_unavailable(req)? {
            resolved = self.skip_unavailable(resolved)?;
            if resolved.is_empty() {
                return Ok(Targets::Nothing);
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

    /// `indexes` without the ones that are no index.
    ///
    /// # Errors
    ///
    /// A failure to open an index other than its being absent.
    pub(super) fn skip_unavailable(&self, indexes: Vec<String>) -> Result<Vec<String>, RestError> {
        // ALLOC: one name per index, at most MAX_INDEXES.
        let mut kept = Vec::with_capacity(indexes.len());
        for index in indexes {
            match self.open_shard(&index, 0, false) {
                Err(Answer::Search(e)) if e.status == 404 => {}
                Err(Answer::Rest(e)) => return Err(e),
                // Another failure surfaces when the search opens it.
                _ => kept.push(index),
            }
        }
        Ok(kept)
    }

    /// The indexes `names` stand for: an alias is its members, an index is
    /// itself, each once and in order. Empty names are skipped.
    ///
    /// # Errors
    ///
    /// A failed alias read, or more than [`MAX_INDEXES`] indexes.
    pub(super) fn expand_names<'n>(
        &self,
        names: impl IntoIterator<Item = &'n str>,
    ) -> Result<Vec<String>, RestError> {
        // ALLOC: one name per entry or member, at most MAX_INDEXES.
        let mut resolved: Vec<String> = Vec::new();
        for name in names.into_iter().filter(|n| !n.is_empty()) {
            if resolved.len() >= MAX_INDEXES {
                return Err(too_many("indexes", MAX_INDEXES));
            }
            let members = self.alias_members(name)?;
            // ALLOC: one name per member or entry.
            let found = members.unwrap_or_else(|| vec![name.to_owned()]);
            for index in found {
                if !resolved.contains(&index) {
                    if resolved.len() >= MAX_INDEXES {
                        return Err(too_many("indexes", MAX_INDEXES));
                    }
                    resolved.push(index);
                }
            }
        }
        Ok(resolved)
    }

    /// The response bytes of searching `indexes` (several) for `source`.
    pub(super) fn search_bytes_many(
        &self,
        indexes: &[String],
        source: &oss_reader::dsl::SearchSource,
        routing: Option<&str>,
        stop: Option<super::Stop<'_>>,
    ) -> Result<Vec<u8>, Answer> {
        #[cfg(feature = "coordinator")]
        {
            let members = self.members_of(indexes, routing)?;
            let visit = Visit {
                shards: members.len(),
                only: None,
                members: Some(&members),
                stop,
            };
            let first = indexes.first().map_or("", String::as_str);
            self.coordinate_bytes(first, visit, source, Instant::now())
        }
        #[cfg(not(feature = "coordinator"))]
        {
            let _ = (indexes, source, routing, stop);
            Err(Answer::Rest(RestError::illegal_argument(
                "a search over several indexes needs the coordinator role",
            )))
        }
    }

    /// The (index, shard) pairs a search over `indexes` visits, `routing`
    /// narrowing each index.
    #[cfg(feature = "coordinator")]
    pub(super) fn members_of(
        &self,
        indexes: &[String],
        routing: Option<&str>,
    ) -> Result<Vec<(String, u32)>, Answer> {
        // ALLOC: one entry per shard of every index searched, at most MAX_SHARDS.
        let mut members: Vec<(String, u32)> = Vec::new();
        for index in indexes {
            let (count, only) = self.shard_plan(index, routing)?;
            if members
                .len()
                .saturating_add(only.as_ref().map_or(count, Vec::len))
                > MAX_SHARDS
            {
                return Err(Answer::Rest(too_many("shards", MAX_SHARDS)));
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
        Ok(members)
    }

    /// Answers a `_search` of what `targets` resolved to.
    pub(super) fn handle_resolved(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
        targets: Targets,
    ) -> Result<(), RestError> {
        match targets {
            Targets::Many(indexes) => self.handle_many(req, resp, &indexes),
            Targets::Nothing => {
                let as_int = req.param("rest_total_hits_as_int").as_deref() == Some("true");
                resp.send(
                    200,
                    oss_rest::JSON,
                    if as_int { NO_HITS_AS_INT } else { NO_HITS },
                )
            }
            Targets::One => Ok(()),
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
            let members = match self.members_of(indexes, routing.as_deref()) {
                Ok(members) => members,
                Err(Answer::Rest(e)) => return Err(e),
                Err(Answer::Search(e)) => return send(resp, &e),
            };
            let visit = Visit {
                shards: members.len(),
                only: None,
                members: Some(&members),
                stop: None,
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

/// `ignore_unavailable`: whether a name that is no index is skipped.
pub(super) fn ignore_unavailable(req: &Request<'_>) -> Result<bool, RestError> {
    parse_flag(req.param("ignore_unavailable").as_deref())
}

/// An `ignore_unavailable` value: absent or `false`, or `true` (an empty one too).
pub(super) fn parse_flag(text: Option<&str>) -> Result<bool, RestError> {
    match text {
        None | Some("false") => Ok(false),
        Some("" | "true") => Ok(true),
        Some(other) => Err(RestError::illegal_argument(format!(
            "Failed to parse value [{other}] as only [true] or [false] are allowed."
        ))),
    }
}
