//! The next `index-N`: a [`RepositoryData`] with one snapshot added, and its
//! JSON in the field order a stock node writes.

use super::repository_data::{RepositoryData, StockIndex, StockSnapshot, is_blob_name};
use crate::error::{FormatError, corrupt};
use crate::json_write::push_string;

const WHAT: &str = "stock repository index blob";

/// One index of a snapshot being added.
#[derive(Debug, Clone, Copy)]
pub struct NewIndex<'a> {
    /// The index's name.
    pub name: &'a str,
    /// The id its blobs are under.
    pub id: &'a str,
    /// The key of this snapshot's metadata generation (`<uuid>-_na_-1-1-1`).
    pub metadata_identifier: &'a str,
    /// The blob id that generation is named by.
    pub blob_id: &'a str,
    /// The current generation of each shard's blob list.
    pub shard_generations: &'a [String],
}

/// A snapshot being added.
#[derive(Debug, Clone, Copy)]
pub struct NewSnapshot<'a> {
    /// The name a restore asks for.
    pub name: &'a str,
    /// The uuid its blobs are named by.
    pub uuid: &'a str,
    /// The state code (1 is `SUCCESS`).
    pub state: i64,
    /// The version recorded as having taken it.
    pub version: &'a str,
    /// The indices it holds.
    pub indices: &'a [NewIndex<'a>],
}

impl RepositoryData {
    /// This data with `new` added; every earlier snapshot, index id and
    /// generation is kept, and a held index gains the snapshot and its new
    /// shard generations.
    pub fn with_snapshot(&self, new: &NewSnapshot<'_>) -> Result<Self, FormatError> {
        if self
            .snapshots
            .iter()
            .any(|s| s.name == new.name || s.uuid == new.uuid)
        {
            return Err(corrupt(WHAT, "a snapshot of that name or uuid is held"));
        }
        let ids = [new.uuid, new.version];
        if !is_blob_name(new.uuid) || ids.iter().any(|s| s.is_empty()) {
            return Err(corrupt(
                WHAT,
                "a snapshot uuid that is not safe in a blob key",
            ));
        }
        // ALLOC: a copy of the repository's lists, bounded by its entries.
        let mut next = self.clone();
        let mut metadata = Vec::new(); // ALLOC: one pair an index of the snapshot.
        for index in new.indices {
            let safe = [index.id, index.metadata_identifier, index.blob_id];
            if index.name.is_empty() || !safe.iter().all(|s| is_blob_name(s)) {
                return Err(corrupt(WHAT, "an index id that is not safe in a blob key"));
            }
            if index.shard_generations.iter().any(|g| !is_blob_name(g)) {
                return Err(corrupt(
                    WHAT,
                    "a shard generation that is not safe in a blob key",
                ));
            }
            // ALLOC: one generation a shard.
            let gens: Vec<Option<String>> = index
                .shard_generations
                .iter()
                .map(|g| Some(g.clone()))
                .collect(); // ALLOC: one generation a shard.
            match next.indices.iter_mut().find(|i| i.name == index.name) {
                Some(held) if held.id != index.id => {
                    return Err(corrupt(WHAT, "an index name held under another id"));
                }
                Some(held) => {
                    held.snapshots.push(new.uuid.to_owned()); // ALLOC: one uuid.
                    held.shard_generations = gens;
                }
                None => next.indices.push(StockIndex {
                    name: index.name.to_owned(), // ALLOC: one name an index.
                    id: index.id.to_owned(),     // ALLOC: one id an index.
                    shard_path_type: 0,
                    snapshots: vec![new.uuid.to_owned()], // ALLOC: one uuid.
                    shard_generations: gens,
                }),
            }
            // ALLOC: one pair an index of the snapshot.
            metadata.push((index.id.to_owned(), index.metadata_identifier.to_owned()));
            if !next
                .identifiers
                .iter()
                .any(|(g, _)| g == index.metadata_identifier)
            {
                // ALLOC: one pair an index of the snapshot.
                next.identifiers.push((
                    index.metadata_identifier.to_owned(),
                    index.blob_id.to_owned(),
                ));
            }
        }
        next.snapshots.push(StockSnapshot {
            name: new.name.to_owned(), // ALLOC: one name.
            uuid: new.uuid.to_owned(), // ALLOC: one uuid.
            state: Some(new.state),
            version: Some(new.version.to_owned()), // ALLOC: one version.
            metadata,
        });
        Ok(next)
    }

    /// The `index-N` JSON, appended to `out`, in the order a stock node
    /// writes it. A generation not written is refused: the take that carries
    /// one forward has nothing a restore could read it by.
    pub fn to_json(&self, out: &mut Vec<u8>) -> Result<(), FormatError> {
        let base = out.len();
        let done = self.write_json(out);
        if done.is_err() {
            out.truncate(base);
        }
        done
    }

    fn write_json(&self, out: &mut Vec<u8>) -> Result<(), FormatError> {
        out.extend_from_slice(b"{\"snapshots\":[");
        for (n, s) in self.snapshots.iter().enumerate() {
            if n > 0 {
                out.push(b',');
            }
            out.extend_from_slice(b"{\"name\":");
            push_string(out, &s.name);
            out.extend_from_slice(b",\"uuid\":");
            push_string(out, &s.uuid);
            if let Some(state) = s.state {
                out.extend_from_slice(format!(",\"state\":{state}").as_bytes()); // ALLOC: one number.
            }
            out.extend_from_slice(b",\"index_metadata_lookup\":{");
            for (m, (id, identifier)) in s.metadata.iter().enumerate() {
                if m > 0 {
                    out.push(b',');
                }
                push_string(out, id);
                out.push(b':');
                push_string(out, identifier);
            }
            out.push(b'}');
            if let Some(v) = &s.version {
                out.extend_from_slice(b",\"version\":");
                push_string(out, v);
            }
            out.push(b'}');
        }
        out.extend_from_slice(b"],\"indices\":{");
        for (n, i) in self.indices.iter().enumerate() {
            if n > 0 {
                out.push(b',');
            }
            push_string(out, &i.name);
            out.extend_from_slice(b":{\"id\":");
            push_string(out, &i.id);
            out.extend_from_slice(format!(",\"shard_path_type\":{}", i.shard_path_type).as_bytes()); // ALLOC: one number.
            out.extend_from_slice(b",\"snapshots\":[");
            for (m, u) in i.snapshots.iter().enumerate() {
                if m > 0 {
                    out.push(b',');
                }
                push_string(out, u);
            }
            out.extend_from_slice(b"],\"shard_generations\":[");
            for (m, g) in i.shard_generations.iter().enumerate() {
                if m > 0 {
                    out.push(b',');
                }
                let g = g
                    .as_deref()
                    .ok_or_else(|| corrupt(WHAT, "a shard generation was not written"))?;
                push_string(out, g);
            }
            out.extend_from_slice(b"]}");
        }
        out.extend_from_slice(b"},\"index_metadata_identifiers\":{");
        for (n, (g, id)) in self.identifiers.iter().enumerate() {
            if n > 0 {
                out.push(b',');
            }
            push_string(out, g);
            out.push(b':');
            push_string(out, id);
        }
        out.extend_from_slice(b"}}");
        Ok(())
    }
}
