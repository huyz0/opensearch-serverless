//! Publishing an exported snapshot into a stock repository: after a
//! shard's files are copied, the shard snapshot, shard generation, index
//! metadata, snapshot info and global metadata blobs are created, then the
//! next `index-N` by a conditional create (the fence), then `index.latest`.
//!
//! A lost race adopts the `index-N` that won as the new base and tries
//! again, rebuilding each shard generation from it and reusing every blob
//! already created; after [`MAX_ATTEMPTS`] the take fails with
//! [`ExportError::Conflict`]. A take that dies after creating `index-N+1`
//! and before the `index.latest` put leaves a repository the next take
//! extends: its create loses, and it adopts what the dead take wrote.

use oss_blob::{CreateOutcome, SliceSource};
use oss_format::{
    StockEnvelopeLimits, StockNewFile, StockNewIndex, StockNewIndexMetadata, StockNewSnapshot,
    StockNewSnapshotInfo, StockRepositoryData, StockShardGeneration, stock_blob_from_json,
    stock_global_metadata_json, stock_index_metadata_json, stock_shard_snapshot_json,
    stock_snapshot_info_json,
};

use crate::pit::new_id;
use crate::stock_export::{ExportError, ExportTarget, ExportedFile};
use crate::stock_read::{StockReadError, StockRepo};

/// How many `index-N` creates a take tries before it fails with a conflict.
pub const MAX_ATTEMPTS: u32 = 16;

/// The state code of a successful snapshot.
const SUCCESS: i64 = 1;

/// The repository as a take starts from it.
#[derive(Debug, Clone, Default)]
pub struct ExportBase {
    generation: Option<u64>,
    data: StockRepositoryData,
}

impl ExportBase {
    /// The id the repository holds `index` under, when it holds it.
    #[must_use]
    pub fn index_id(&self, index: &str) -> Option<&str> {
        self.data
            .indices
            .iter()
            .find(|i| i.name == index)
            .map(|i| i.id.as_str())
    }

    /// Whether a snapshot of `snapshot`'s name or uuid is held.
    fn holds(&self, snapshot: &ExportSnapshot<'_>) -> bool {
        self.data
            .snapshots
            .iter()
            .any(|s| s.name == snapshot.name || s.uuid == snapshot.uuid)
    }

    /// The generation `index.latest` named, none for an empty repository.
    #[must_use]
    pub fn generation(&self) -> Option<u64> {
        self.generation
    }
}

/// Reads where the repository stands. Cost: one GET for `index.latest` and,
/// for a repository with snapshots, one for its `index-N`.
///
/// # Errors
///
/// [`ExportError::Read`] for a blob that is malformed or too large;
/// [`ExportError::Blob`] for a store failure.
pub fn open_export_base(target: &ExportTarget<'_>) -> Result<ExportBase, ExportError> {
    let repo = StockRepo::new(target.store, target.prefix);
    match repo.generation() {
        Err(StockReadError::Missing(_)) => Ok(ExportBase::default()),
        Err(e) => Err(ExportError::Read(e)),
        Ok(generation) => Ok(ExportBase {
            generation: Some(generation),
            data: repo
                .repository_data(generation)
                .map_err(ExportError::Read)?,
        }),
    }
}

/// One shard as the copy left it.
#[derive(Debug, Clone)]
pub struct ExportShard {
    /// The files copied, with their measures.
    pub files: Vec<ExportedFile>,
    /// The commit's version number, recorded as the shard snapshot's
    /// `index_version`.
    pub index_version: u64,
}

/// One index of the snapshot, its shards copied under `id`.
#[derive(Debug, Clone, Copy)]
pub struct ExportIndex<'a> {
    /// The index's name.
    pub name: &'a str,
    /// The id its blobs were copied under.
    pub id: &'a str,
    /// The index's uuid.
    pub uuid: &'a str,
    /// `index.creation_date`, in epoch milliseconds.
    pub creation_date_ms: u64,
    /// The `_doc` mapping as a JSON object, when it has one.
    pub mapping: Option<&'a str>,
    /// The settings to carry.
    pub settings: &'a [(String, String)],
    /// Each shard, in shard order.
    pub shards: &'a [ExportShard],
}

/// A snapshot to publish.
#[derive(Debug, Clone, Copy)]
pub struct ExportSnapshot<'a> {
    /// Its name.
    pub name: &'a str,
    /// Its uuid.
    pub uuid: &'a str,
    /// When the take began, in epoch milliseconds.
    pub start_ms: u64,
    /// When the copy ended.
    pub end_ms: u64,
    /// The indices it holds.
    pub indices: &'a [ExportIndex<'a>],
}

/// What a published snapshot is named by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Published {
    /// The generation of the `index-N` that names it.
    pub generation: u64,
    /// How many `index-N` creates it took.
    pub attempts: u32,
}

/// Creates `key` under the target's prefix from `json` in the envelope of
/// `codec`.
fn put_blob(
    target: &ExportTarget<'_>,
    key: &str,
    json: &[u8],
    codec: &str,
    scratch: &mut [u8],
) -> Result<(), ExportError> {
    // ALLOC: one blob, the size of its JSON.
    let mut blob = Vec::with_capacity(json.len().saturating_add(256));
    stock_blob_from_json(json, codec, &StockEnvelopeLimits::default(), &mut blob)?;
    create(target, key, &blob, scratch)
}

fn create(
    target: &ExportTarget<'_>,
    key: &str,
    bytes: &[u8],
    scratch: &mut [u8],
) -> Result<(), ExportError> {
    let key = [target.prefix, key].concat(); // ALLOC: one key a blob.
    match target
        .store
        .create_if_absent(&key, &mut SliceSource::new(bytes), scratch)?
    {
        CreateOutcome::Created => Ok(()),
        CreateOutcome::AlreadyExists => Err(ExportError::Unsupported(
            "a new blob name was already taken",
        )),
    }
}

/// The files of a shard as the writers take them.
fn new_files(files: &[ExportedFile]) -> Vec<StockNewFile<'_>> {
    // ALLOC: one entry a file, at most the commit's file bound.
    files
        .iter()
        .map(|f| StockNewFile {
            name: &f.name,
            physical_name: &f.physical_name,
            length: f.length,
            checksum: &f.checksum,
            written_by: &f.written_by,
        })
        .collect() // ALLOC: the entries counted above.
}

/// Publishes `snapshot` into the repository `base` described. Cost: per
/// shard one put for its snapshot blob and one for its generation, plus one
/// read of the previous generation when the index is already held; per
/// index one put; four puts for the snapshot, and one read of each
/// generation adopted after a lost race.
///
/// # Errors
///
/// [`ExportError::NameTaken`] when the snapshot's name or uuid is held;
/// [`ExportError::Conflict`] when [`MAX_ATTEMPTS`] creates all lost, or
/// another take gave an index a different id; [`ExportError::Unsupported`]
/// when an index's shard count changed since the repository's own record
/// of it; the writers', the store's and the readers' errors.
pub fn publish_snapshot(
    target: &ExportTarget<'_>,
    base: ExportBase,
    snapshot: &ExportSnapshot<'_>,
    scratch: &mut [u8],
) -> Result<Published, ExportError> {
    if base.holds(snapshot) {
        return Err(ExportError::NameTaken);
    }
    let mut json = Vec::new(); // ALLOC: one blob's JSON at a time, reused.
    let mut metadata: Vec<(String, String, String)> = Vec::new(); // ALLOC: one entry an index.
    for index in snapshot.indices {
        for (n, shard) in index.shards.iter().enumerate() {
            json.clear();
            let files = new_files(&shard.files);
            stock_shard_snapshot_json(
                snapshot.name,
                shard.index_version,
                snapshot.start_ms,
                snapshot.end_ms.saturating_sub(snapshot.start_ms),
                &files,
                &mut json,
            )?;
            let key = format!("indices/{}/{n}/snap-{}.dat", index.id, snapshot.uuid); // ALLOC: one key a shard.
            put_blob(target, &key, &json, "snapshot", scratch)?;
        }
        // ALLOC: one id a shard.
        let allocation_ids = index
            .shards
            .iter()
            .map(|_| new_id())
            .collect::<Result<Vec<_>, _>>()?;
        json.clear();
        stock_index_metadata_json(
            &StockNewIndexMetadata {
                name: index.name,
                uuid: index.uuid,
                number_of_shards: u32::try_from(index.shards.len()).unwrap_or(u32::MAX),
                creation_date_ms: index.creation_date_ms,
                mapping: index.mapping,
                settings: index.settings,
                allocation_ids: &allocation_ids,
            },
            &mut json,
        )?;
        let blob = new_id()?;
        let key = format!("indices/{}/meta-{blob}.dat", index.id); // ALLOC: one key an index.
        put_blob(target, &key, &json, "index-metadata", scratch)?;
        // ALLOC: one entry an index.
        metadata.push((
            index.id.to_owned(),
            format!("{}-{}-1-1-1", index.uuid, snapshot.uuid),
            blob,
        ));
    }
    let names: Vec<&str> = snapshot.indices.iter().map(|i| i.name).collect(); // ALLOC: one name an index.
    let shards: u64 = snapshot
        .indices
        .iter()
        .map(|i| u64::try_from(i.shards.len()).unwrap_or(u64::MAX))
        .sum();
    json.clear();
    stock_snapshot_info_json(
        &StockNewSnapshotInfo {
            name: snapshot.name,
            uuid: snapshot.uuid,
            indices: &names,
            start_time_ms: snapshot.start_ms,
            end_time_ms: snapshot.end_ms,
            total_shards: shards,
            successful_shards: shards,
        },
        &mut json,
    )?;
    put_blob(
        target,
        &format!("snap-{}.dat", snapshot.uuid), // ALLOC: one key.
        &json,
        "snapshot",
        scratch,
    )?;
    json.clear();
    stock_global_metadata_json(&mut json)?;
    put_blob(
        target,
        &format!("meta-{}.dat", snapshot.uuid), // ALLOC: one key.
        &json,
        "metadata",
        scratch,
    )?;
    fence(target, base, snapshot, &metadata, scratch)
}

/// Creates the next `index-N` and then `index.latest`, adopting the winner of
/// each lost race.
fn fence(
    target: &ExportTarget<'_>,
    mut base: ExportBase,
    snapshot: &ExportSnapshot<'_>,
    metadata: &[(String, String, String)],
    scratch: &mut [u8],
) -> Result<Published, ExportError> {
    let repo = StockRepo::new(target.store, target.prefix);
    for attempt in 1..=MAX_ATTEMPTS {
        if base.holds(snapshot) {
            return Err(ExportError::NameTaken);
        }
        // ALLOC: one generation a shard, per index.
        let mut generations: Vec<Vec<String>> = Vec::with_capacity(snapshot.indices.len());
        for index in snapshot.indices {
            if base.index_id(index.name).is_some_and(|id| id != index.id) {
                return Err(ExportError::Conflict);
            }
            generations.push(shard_generations(
                target, &repo, &base, snapshot, index, scratch,
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
        let number = base.generation.map_or(0, |g| g.saturating_add(1));
        let key = format!("index-{number}"); // ALLOC: one key.
        let full = [target.prefix, &key].concat(); // ALLOC: one key.
        match target
            .store
            .create_if_absent(&full, &mut SliceSource::new(&bytes), scratch)?
        {
            CreateOutcome::Created => {
                let latest = [target.prefix, "index.latest"].concat(); // ALLOC: one key.
                let raw = i64::try_from(number).unwrap_or(i64::MAX).to_be_bytes();
                target
                    .store
                    .put(&latest, &mut SliceSource::new(&raw), scratch)?;
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

/// Creates each shard's next generation blob of `index`: the base's, when it
/// holds the index, with the snapshot added.
fn shard_generations(
    target: &ExportTarget<'_>,
    repo: &StockRepo<'_>,
    base: &ExportBase,
    snapshot: &ExportSnapshot<'_>,
    index: &ExportIndex<'_>,
    scratch: &mut [u8],
) -> Result<Vec<String>, ExportError> {
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
        let previous = match held.and_then(|g| g.get(n)).and_then(Option::as_deref) {
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
        out.push(id);
    }
    Ok(out)
}
