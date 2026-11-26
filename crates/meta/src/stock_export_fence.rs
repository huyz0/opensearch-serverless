//! The fence of a publish: the next `index-N` by conditional create,
//! adopting the winner of each lost race, and then `index.latest`.

use std::collections::HashMap;

use oss_blob::{CreateOutcome, SliceSource};
use oss_format::{StockNewIndex, StockNewSnapshot, StockShardGeneration};

use crate::pit::new_id;
use crate::stock_export::{ExportError, ExportTarget};
use crate::stock_export_take::{
    ExportBase, ExportIndex, ExportSnapshot, Published, new_files, put_blob,
};
use crate::stock_read::{MAX_INDEX_N_BYTES, StockRepo};

/// How many `index-N` creates a take tries before it fails with a conflict.
pub const MAX_ATTEMPTS: u32 = 16;

/// The state code of a successful snapshot.
const SUCCESS: i64 = 1;

/// Creates the next `index-N` and then `index.latest`, adopting the winner of
/// each lost race.
pub(crate) fn fence(
    target: &ExportTarget<'_>,
    mut base: ExportBase,
    snapshot: &ExportSnapshot<'_>,
    metadata: &[(String, String, String)],
    scratch: &mut [u8],
) -> Result<Published, ExportError> {
    let repo = StockRepo::new(target.store, target.prefix);
    // ALLOC: one entry a shard, filled once.
    let mut written: HashMap<(usize, usize), (Option<String>, String)> = HashMap::new();
    for attempt in 1..=MAX_ATTEMPTS {
        if base.holds(snapshot) {
            return Err(ExportError::NameTaken);
        }
        // ALLOC: one generation a shard, per index.
        let mut generations: Vec<Vec<String>> = Vec::with_capacity(snapshot.indices.len());
        for (at, index) in snapshot.indices.iter().enumerate() {
            if base.index_id(index.name).is_some_and(|id| id != index.id) {
                return Err(ExportError::Conflict);
            }
            let shared = Shared {
                target,
                repo: &repo,
                base: &base,
                snapshot,
            };
            generations.push(shard_generations(
                &shared,
                at,
                index,
                &mut written,
                scratch,
            )?);
        }
        // ALLOC: one entry an index.
        let new_indices: Vec<StockNewIndex<'_>> = snapshot
            .indices
            .iter()
            .zip(&generations)
            .zip(metadata)
            .map(|((index, gens), (_, identifier, blob))| StockNewIndex {
                name: index.name,
                id: index.id,
                metadata_identifier: identifier,
                blob_id: blob,
                shard_generations: gens,
            })
            .collect(); // ALLOC: the entries counted above.
        let next = base.data.with_snapshot(&StockNewSnapshot {
            name: snapshot.name,
            uuid: snapshot.uuid,
            state: SUCCESS,
            version: "3.10.0",
            indices: &new_indices,
        })?;
        let mut bytes = Vec::new(); // ALLOC: one index-N, at most its read bound.
        next.to_json(&mut bytes)?;
        if !within_read_bound(bytes.len()) {
            return Err(ExportError::Unsupported(
                "the repository's index blob would pass the size a restore reads",
            ));
        }
        let number = base.generation.map_or(0, |g| g.saturating_add(1));
        let key = format!("index-{number}"); // ALLOC: one key.
        let full = [target.prefix, &key].concat(); // ALLOC: one key.
        match target
            .store
            .create_if_absent(&full, &mut SliceSource::new(&bytes), scratch)?
        {
            CreateOutcome::Created => {
                put_latest(target, number, scratch)?;
                return Ok(Published {
                    generation: number,
                    attempts: attempt,
                });
            }
            CreateOutcome::AlreadyExists => {
                base = ExportBase {
                    generation: Some(number),
                    data: repo.repository_data(number).map_err(ExportError::Read)?,
                };
            }
        }
    }
    Err(ExportError::Conflict)
}

/// What each attempt reads from.
#[derive(Clone, Copy)]
struct Shared<'a, 'b> {
    target: &'a ExportTarget<'b>,
    repo: &'a StockRepo<'b>,
    base: &'a ExportBase,
    snapshot: &'a ExportSnapshot<'a>,
}

/// Creates each shard's next generation blob of `index`: the base's, when it
/// holds the index, with the snapshot added.
fn shard_generations(
    shared: &Shared<'_, '_>,
    at: usize,
    index: &ExportIndex<'_>,
    written: &mut HashMap<(usize, usize), (Option<String>, String)>,
    scratch: &mut [u8],
) -> Result<Vec<String>, ExportError> {
    let Shared {
        target,
        repo,
        base,
        snapshot,
    } = *shared;
    let held = base
        .data
        .indices
        .iter()
        .find(|i| i.name == index.name)
        .map(|i| i.shard_generations.as_slice());
    if held.is_some_and(|g| g.len() != index.shards.len()) {
        return Err(ExportError::Unsupported(
            "the index's shard count differs from the repository's record of it",
        ));
    }
    let mut out = Vec::with_capacity(index.shards.len()); // ALLOC: one generation a shard.
    let mut json = Vec::new(); // ALLOC: one generation's JSON at a time, reused.
    for (n, shard) in index.shards.iter().enumerate() {
        let held_generation = held.and_then(|g| g.get(n)).and_then(Option::as_deref);
        // A generation already put on top of the same one is reused.
        if let Some((_, id)) = written
            .get(&(at, n))
            .filter(|(on, _)| on.as_deref() == held_generation)
        {
            out.push(id.clone()); // ALLOC: one id a shard.
            continue;
        }
        let previous = match held_generation {
            Some(generation) => {
                let key = format!("indices/{}/{n}/index-{generation}", index.id); // ALLOC: one key.
                let read = repo.json(&key, "snapshots").map_err(ExportError::Read)?;
                StockShardGeneration::parse(&read)?
            }
            None => StockShardGeneration::default(),
        };
        let next = previous.with_snapshot(snapshot.name, &new_files(&shard.files))?;
        json.clear();
        next.to_json(&mut json)?;
        let id = new_id()?;
        let key = format!("indices/{}/{n}/index-{id}", index.id); // ALLOC: one key.
        put_blob(target, &key, &json, "snapshots", scratch)?;
        // ALLOC: one entry a shard.
        written.insert((at, n), (held_generation.map(str::to_owned), id.clone()));
        out.push(id);
    }
    Ok(out)
}

/// Whether an `index-N` of `len` bytes is one a restore will read.
fn within_read_bound(len: usize) -> bool {
    len <= MAX_INDEX_N_BYTES
}

/// Points `index.latest` at `number`, unless `index-<number + 1>` exists:
/// the take that created it extended this one and puts a newer generation,
/// which a put of this one could move back. A take that loses that last
/// race leaves `index.latest` naming an older generation that restores; the
/// next take adopts the newer `index-N` and moves it on. Cost: one HEAD
/// and, when nothing newer exists, one put.
fn put_latest(
    target: &ExportTarget<'_>,
    number: u64,
    scratch: &mut [u8],
) -> Result<(), ExportError> {
    let newer = format!("{}index-{}", target.prefix, number.saturating_add(1)); // ALLOC: one key.
    if target.store.head(&newer)?.is_some() {
        return Ok(());
    }
    let latest = [target.prefix, "index.latest"].concat(); // ALLOC: one key.
    let raw = i64::try_from(number).unwrap_or(i64::MAX).to_be_bytes();
    target
        .store
        .put(&latest, &mut SliceSource::new(&raw), scratch)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn created(store: &oss_blob::Emulator, key: &str) -> bool {
        use oss_blob::BlobStore;
        store.head(key).unwrap().is_some()
    }

    #[test]
    fn index_latest_is_put_unless_a_newer_index_blob_exists() {
        use oss_blob::{BlobStore, Emulator};
        let store = Emulator::new(1 << 20, 1);
        let target = ExportTarget {
            store: &store,
            prefix: "r/",
        };
        let mut buf = [0u8; 64];
        // Nothing newer: the put lands and names 3.
        put_latest(&target, 3, &mut buf).unwrap();
        let mut got = Vec::new();
        store
            .get(
                "r/index.latest",
                oss_blob::ByteRange::Full,
                &mut buf,
                &mut |c: &[u8]| {
                    got.extend_from_slice(c);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(got, 3i64.to_be_bytes());
        // index-5 exists: a take for 4 leaves index.latest alone.
        store
            .put("r/index-5", &mut SliceSource::new(b"{}"), &mut buf)
            .unwrap();
        put_latest(&target, 4, &mut buf).unwrap();
        let mut again = Vec::new();
        store
            .get(
                "r/index.latest",
                oss_blob::ByteRange::Full,
                &mut buf,
                &mut |c: &[u8]| {
                    again.extend_from_slice(c);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(again, 3i64.to_be_bytes());
        assert!(created(&store, "r/index-5"));
    }

    #[test]
    fn the_read_bound_holds_exactly() {
        assert!(within_read_bound(MAX_INDEX_N_BYTES));
        assert!(!within_read_bound(MAX_INDEX_N_BYTES + 1));
    }
}
