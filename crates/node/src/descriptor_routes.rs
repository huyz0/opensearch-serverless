//! A writer's view of its indexes, read from the descriptors.
//!
//! Ported from the routing reads of `serverless/shell/src/main/java/org/opensearch/serverless/metadata/ServerlessIndexResolver.java`
//! (`IndexMetadata`'s routing partition size, routing shards and the mapping's
//! `_routing.required`), over `oss-meta`'s descriptor store.

use std::sync::Arc;

use oss_meta::{DescriptorStore, Target};
use oss_rest::Exception;
use oss_writer::bulk::{IndexInfo, ItemResult, Owner, Planned, WriteBlock};
use oss_writer::mapping::Mapping;
use oss_writer::mapping::json::{Json, parse};

use crate::index_cache::{IndexCache, Lookup};
use crate::writer::Routes;

/// Routes whose indexes are the descriptors' and whose shards are all this
/// node's.
pub struct DescriptorRoutes {
    descriptors: DescriptorStore,
    buffer_bytes: usize,
    cache: Option<Arc<IndexCache>>,
}

impl DescriptorRoutes {
    /// Routes over `descriptors`, each lookup reading through a buffer of
    /// `buffer_bytes` (a descriptor that does not fit is no index).
    pub fn new(descriptors: DescriptorStore, buffer_bytes: usize) -> Self {
        Self {
            descriptors,
            buffer_bytes,
            cache: None,
        }
    }

    /// Keeps the indexes a lookup reads in `cache`, shared with the other
    /// routes of the node, which confirms them by the change stripes.
    #[must_use]
    pub fn with_cache(mut self, cache: Arc<IndexCache>) -> Self {
        self.cache = Some(cache);
        self
    }
}

/// The value of `key` in settings JSON, flat (`"index.a.b"`) or nested
/// (`{"index":{"a":{"b":..}}}`), as a number.
fn number(settings: &[(String, Json)], key: &str) -> Option<u32> {
    let value = settings
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
        .or_else(|| nested(settings, key))?;
    match value {
        Json::Num(text) | Json::Str(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// Whether the boolean setting `key` is `true`, as a bool or as text, flat or nested.
fn is_on(settings: &[(String, Json)], key: &str) -> bool {
    let value = settings
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
        .or_else(|| nested(settings, key));
    match value {
        Some(Json::Bool(on)) => *on,
        Some(Json::Str(text)) => text.trim() == "true",
        _ => false,
    }
}

/// The write block the index's settings name.
fn write_block(settings: &[(String, Json)]) -> WriteBlock {
    if is_on(settings, "index.blocks.write") || is_on(settings, "index.blocks.read_only") {
        WriteBlock::Full
    } else if is_on(settings, "index.blocks.read_only_allow_delete") {
        WriteBlock::AllowDelete
    } else {
        WriteBlock::None
    }
}

/// The pipeline id `key` names in settings JSON, flat or nested; none when
/// unset or empty.
fn text(settings: &[(String, Json)], key: &str) -> Option<String> {
    let value = settings
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
        .or_else(|| nested(settings, key))?;
    match value {
        // ALLOC: one id, only for an index that sets one.
        Json::Str(id) if !id.is_empty() => Some(id.to_string()),
        _ => None,
    }
}

fn nested<'a>(settings: &'a [(String, Json)], key: &str) -> Option<&'a Json> {
    let mut members = settings;
    let mut parts = key.split('.').peekable();
    while let Some(part) = parts.next() {
        let (_, value) = members.iter().find(|(k, _)| k == part)?;
        if parts.peek().is_none() {
            return Some(value);
        }
        match value {
            Json::Obj(inner) => members = inner,
            _ => return None,
        }
    }
    None
}

impl Routes for DescriptorRoutes {
    fn index(&self, name: &str) -> Option<IndexInfo> {
        let stamp = match self.cache.as_deref().map(|c| c.lookup(name)) {
            Some(Lookup::Hit(info)) => return Some(info),
            Some(Lookup::Miss(stamp)) => Some(stamp),
            Some(Lookup::Uncached) | None => None,
        };
        let info = self.read(name)?;
        if let (Some(cache), Some(stamp)) = (&self.cache, stamp) {
            cache.keep(name, &info, stamp);
        }
        Some(info)
    }

    fn owner(&self, _: &IndexInfo, _: u32) -> Owner {
        Owner::Local
    }

    fn forward(&self, _: &str, _: &IndexInfo, _: u32, group: &[Planned<'_>]) -> Vec<ItemResult> {
        group
            .iter()
            .map(|_| ItemResult::Failed {
                status: 503,
                // ALLOC: error path; one exception an item.
                error: Exception::new("no_owner", "this node forwards to no other node"),
            })
            .collect()
    }
}

impl DescriptorRoutes {
    /// The index `name` as its descriptor says: one register read.
    fn read(&self, name: &str) -> Option<IndexInfo> {
        // ALLOC: one buffer a lookup, buffer_bytes.
        let mut buf = vec![0u8; self.buffer_bytes];
        let held = self.descriptors.resolve(name, &mut buf).ok()?;
        let Target::Index(record) = held.target else {
            return None;
        };
        // An index a restore has not finished takes no write.
        if oss_format::is_restoring(record.settings) {
            return None;
        }
        let settings = match parse(record.settings) {
            Ok(Json::Obj(members)) => members,
            _ => Vec::new(),
        };
        let shards = record.number_of_shards;
        let routing_required = record
            .mapping
            .as_deref()
            .and_then(|text| Mapping::parse(text.as_bytes()).ok())
            .is_some_and(|m| m.routing_required());
        Some(IndexInfo {
            // ALLOC: two names a lookup.
            name: record.name.to_string(),
            uuid: record.uuid.to_string(),
            shards,
            partition_size: number(&settings, "index.routing_partition_size").unwrap_or(1),
            routing_shards: number(&settings, "index.number_of_routing_shards").unwrap_or(shards),
            routing_required,
            write_block: write_block(&settings),
            default_pipeline: text(&settings, "index.default_pipeline"),
            final_pipeline: text(&settings, "index.final_pipeline"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oss_blob::{BlobStore, Emulator};
    use oss_format::{IndexRecord, mark_restoring};
    use oss_meta::DeleteMode;

    #[test]
    fn an_index_a_restore_has_not_finished_takes_no_write() {
        let store: Arc<dyn BlobStore> = Arc::new(Emulator::new(1 << 20, 1));
        let descriptors = DescriptorStore::new(store, "", DeleteMode::Conditional).unwrap();
        let whole = IndexRecord::new("whole", "u1", 1);
        descriptors.create(&whole, 1, &mut vec![0u8; 4096]).unwrap();
        let marked = mark_restoring(None).unwrap();
        let mut half = IndexRecord::new("half", "u2", 1);
        half.settings = &marked;
        descriptors.create(&half, 1, &mut vec![0u8; 4096]).unwrap();
        let routes = DescriptorRoutes::new(descriptors, 4096);
        assert!(routes.index("whole").is_some());
        assert!(routes.index("half").is_none());
    }
}
