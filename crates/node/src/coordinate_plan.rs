//! Which shards a search visits: the routing narrowing and the sharded answer.

use std::time::Instant;

use oss_blob::BlobStore;
use oss_reader::dsl::SearchSource;
use oss_rest::{Response, RestError};

use super::{Answer, SearchService, Visit};

impl<S: BlobStore + 'static> SearchService<S> {
    /// What a search of `index` visits: its shard count, and the shards
    /// `routing` narrows it to on an index of several. A routing partition
    /// or a shard mode visits every shard.
    pub(in super::super) fn shard_plan(
        &self,
        index: &str,
        routing: Option<&str>,
    ) -> Result<(usize, Option<Vec<u32>>), Answer> {
        let held = self.open_shard(index, 0, false)?;
        let shards = held.shard.meta.number_of_shards;
        let count = usize::try_from(shards).unwrap_or(usize::MAX);
        let values = oss_cluster::routing::search_routings(routing);
        if shards < 2 || values.is_empty() {
            return Ok((count, None));
        }
        let setting = |key: &str, default: u32| match held.shard.meta.settings.get(key) {
            Some(oss_format::Setting::Value(text)) => text.trim().parse().unwrap_or(default),
            _ => default,
        };
        let layout = oss_cluster::routing::DocumentRouting::legacy_scaled(
            shards,
            setting("index.routing_partition_size", 1),
            setting("index.number_of_routing_shards", shards),
        );
        // ALLOC: one number per shard visited.
        let only: Option<Vec<u32>> = layout
            .ok()
            .and_then(|l| l.shards_for_search(&values))
            .map(|set| set.into_iter().collect());
        Ok((only.as_ref().map_or(count, Vec::len), only))
    }

    /// Answers `source` over the shards `plan` (from [`Self::shard_plan`])
    /// visits, refusing a collapse over them.
    pub(in super::super) fn handle_planned(
        &self,
        resp: &mut Response<'_>,
        index: &str,
        plan: &(usize, Option<Vec<u32>>),
        source: &SearchSource,
        started: Instant,
    ) -> Result<(), RestError> {
        if source.collapse.is_some() {
            return Err(RestError::illegal_argument(
                "[collapse] over an index of several shards is not supported by this node yet",
            ));
        }
        let visit = Visit {
            shards: plan.0,
            only: plan.1.as_deref(),
            members: None,
        };
        self.handle_sharded(resp, index, visit, source, started)
    }
}
