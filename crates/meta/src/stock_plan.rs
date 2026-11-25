//! Planning a stock restore: the four read rounds, every refusal and every
//! bound, all before the first write.
//!
//! Round 1 reads `index.latest`, round 2 `index-N`, round 3 the snapshot's
//! own blob and one metadata blob for each requested index, round 4 one
//! shard blob for each shard. Each round fans out at most `fanout` reads at
//! a time. The metadata of an index nobody asked for is never read.

use oss_format::{
    StockIndex, StockIndexMetadata, StockNative, StockRepositoryData, StockShardSnapshot,
    StockSnapshotInfo,
};

use crate::stock_fan::fan_out;
use crate::stock_read::{StockReadError, StockRepo};

/// The most indices one restore names.
pub const MAX_INDICES: usize = 100;
/// The most shards one restore copies.
pub const MAX_SHARDS: usize = 4_096;
/// The most file entries across the shards of one restore.
pub const MAX_FILE_ENTRIES: usize = 100_000;

/// The bounds of one restore.
#[derive(Debug, Clone, Copy)]
pub struct StockLimits {
    /// The most indices.
    pub indices: usize,
    /// The most shards.
    pub shards: usize,
    /// The most file entries across the shards.
    pub file_entries: usize,
}

impl Default for StockLimits {
    fn default() -> Self {
        Self {
            indices: MAX_INDICES,
            shards: MAX_SHARDS,
            file_entries: MAX_FILE_ENTRIES,
        }
    }
}

/// What a plan decides with, from the node.
pub struct StockRules<'a> {
    /// The bounds of the restore.
    pub limits: StockLimits,
    /// The most reads in flight in a round (at least 1).
    pub fanout: usize,
    /// Whether a Rust index keeps a setting key.
    pub keeps: &'a dyn Fn(&str) -> bool,
    /// Whether a create would accept this mapping (the JSON object).
    pub mapping_ok: &'a dyn Fn(&str) -> bool,
}

/// Why a restore cannot be planned.
#[derive(Debug)]
pub enum StockPlanError {
    /// The repository holds no snapshot of that name.
    UnknownSnapshot(String),
    /// The snapshot holds no index of that name.
    UnknownIndex(String),
    /// A snapshot or index the restore does not carry (501).
    Unsupported(String),
    /// A restore past a bound (400 `snapshot_too_large`).
    TooLarge(&'static str),
    /// The repository's own data contradicts itself (400).
    Corrupt(&'static str),
    /// A blob could not be read.
    Read(StockReadError),
}

impl std::fmt::Display for StockPlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownSnapshot(n) => write!(f, "the repository has no snapshot [{n}]"),
            Self::UnknownIndex(n) => write!(f, "the snapshot has no index [{n}]"),
            Self::Unsupported(why) => write!(f, "{why}"),
            Self::TooLarge(what) => write!(f, "the restore names too many {what}"),
            Self::Corrupt(why) => write!(f, "the repository is inconsistent: {why}"),
            Self::Read(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for StockPlanError {}

impl From<StockReadError> for StockPlanError {
    fn from(e: StockReadError) -> Self {
        Self::Read(e)
    }
}

/// One index to restore.
#[derive(Debug)]
pub struct PlannedIndex {
    /// The index's name when it was snapshotted.
    pub name: String,
    /// The id its blobs are under.
    pub id: String,
    /// What it becomes.
    pub native: StockNative,
    /// Each shard's snapshot, in shard order.
    pub shards: Vec<StockShardSnapshot>,
}

/// A restore, read and checked, ready to write.
#[derive(Debug)]
pub struct StockPlan {
    /// The snapshot's uuid.
    pub uuid: String,
    /// The indices, in repository order.
    pub indices: Vec<PlannedIndex>,
}

/// The repository's data: rounds 1 and 2.
///
/// # Errors
///
/// A [`StockReadError`] for a missing, hostile or corrupt blob.
pub fn load(repo: &StockRepo<'_>) -> Result<StockRepositoryData, StockReadError> {
    repo.repository_data(repo.generation()?)
}

/// Rounds 3 and 4 for `snapshot`'s `wanted` indices (all of them for
/// `None`), then every check, before anything is written.
///
/// # Errors
///
/// A [`StockPlanError`]; no write has happened for any of them.
pub fn plan(
    repo: &StockRepo<'_>,
    data: &StockRepositoryData,
    snapshot: &str,
    wanted: Option<&[String]>,
    rules: &StockRules<'_>,
) -> Result<StockPlan, StockPlanError> {
    let snap = data
        .snapshot(snapshot)
        // ALLOC: error path; one name.
        .ok_or_else(|| StockPlanError::UnknownSnapshot(snapshot.to_owned()))?;
    let held: Vec<&StockIndex> = data.indices_of(snap).collect();
    let chosen = choose(&held, wanted)?;
    if chosen.len() > rules.limits.indices {
        return Err(StockPlanError::TooLarge("indices"));
    }
    if chosen.iter().any(|i| i.shard_path_type != 0) {
        return Err(StockPlanError::Unsupported(
            "the snapshot uses a shard layout this node does not read".to_owned(), // ALLOC: error path; one message.
        ));
    }
    // Round 3: the snapshot's own blob, and one metadata blob for each index.
    let mut jobs: Vec<Option<(&str, &str)>> = vec![None]; // ALLOC: one job per index.
    for index in &chosen {
        let id = data
            .metadata_blob_id(snap, index)
            .ok_or(StockPlanError::Corrupt("an index has no metadata blob"))?;
        jobs.push(Some((index.id.as_str(), id)));
    }
    let answers = fan_out(rules.fanout, &jobs, |job| match job {
        None => repo.snapshot_info(&snap.uuid).map(Round3::Info),
        Some((index, meta)) => repo.index_metadata(index, meta).map(Round3::Meta),
    });
    let (info, metas) = split_round3(answers)?;
    if let Some(why) = info.refusal() {
        return Err(StockPlanError::Unsupported(why.to_owned())); // ALLOC: error path; one message.
    }
    // ALLOC: one native form for each index.
    let natives = metas
        .iter()
        .map(|meta| native(meta, rules))
        .collect::<Result<Vec<_>, _>>()?;
    let shards = natives.iter().fold(0usize, |n, native| {
        n.saturating_add(native.number_of_shards as usize)
    });
    if shards > rules.limits.shards {
        return Err(StockPlanError::TooLarge("shards"));
    }
    // Round 4: one shard blob for each shard.
    let mut jobs: Vec<(&str, u32)> = Vec::new(); // ALLOC: one job per shard, at most MAX_SHARDS.
    for (index, native) in chosen.iter().zip(&natives) {
        for shard in 0..native.number_of_shards {
            jobs.push((index.id.as_str(), shard));
        }
    }
    let answers = fan_out(rules.fanout, &jobs, |(index, shard)| {
        repo.shard_snapshot(index, *shard, &snap.uuid)
    });
    let mut files = 0usize;
    // ALLOC: one snapshot for each shard, files bounded by MAX_FILE_ENTRIES.
    let mut read = Vec::new();
    for answer in answers {
        let shard = answer.ok_or(StockPlanError::Corrupt("a read did not finish"))??;
        files = files.saturating_add(shard.files.len());
        if files > rules.limits.file_entries {
            return Err(StockPlanError::TooLarge("file entries"));
        }
        read.push(shard);
    }
    let mut read = read.into_iter();
    // ALLOC: one entry for each index.
    let indices = chosen
        .iter()
        .zip(natives)
        .map(|(index, native)| PlannedIndex {
            name: index.name.clone(), // ALLOC: one name for each index.
            id: index.id.clone(),     // ALLOC: one id for each index.
            shards: read
                .by_ref()
                .take(native.number_of_shards as usize)
                .collect(), // ALLOC: one shard list for each index.
            native,
        })
        .collect(); // ALLOC: one entry for each index.
    Ok(StockPlan {
        uuid: snap.uuid.clone(), // ALLOC: one uuid.
        indices,
    })
}

enum Round3 {
    Info(StockSnapshotInfo),
    Meta(StockIndexMetadata),
}

type Answer = Option<Result<Round3, StockReadError>>;

/// The snapshot's info (the first answer) and each index's metadata.
fn split_round3(
    answers: Vec<Answer>,
) -> Result<(StockSnapshotInfo, Vec<StockIndexMetadata>), StockPlanError> {
    let mut info = None;
    let mut metas = Vec::new(); // ALLOC: one entry for each index.
    for answer in answers {
        match answer.ok_or(StockPlanError::Corrupt("a read did not finish"))?? {
            Round3::Info(i) => info = Some(i),
            Round3::Meta(m) => metas.push(m),
        }
    }
    let info = info.ok_or(StockPlanError::Corrupt("the snapshot has no blob"))?;
    Ok((info, metas))
}

/// The indices `wanted` names, each of which the snapshot must hold.
fn choose<'a>(
    held: &[&'a StockIndex],
    wanted: Option<&[String]>,
) -> Result<Vec<&'a StockIndex>, StockPlanError> {
    let Some(wanted) = wanted else {
        return Ok(held.to_vec()); // ALLOC: one pointer for each index.
    };
    for name in wanted {
        if !held.iter().any(|i| i.name == *name) {
            // ALLOC: error path; one name.
            return Err(StockPlanError::UnknownIndex(name.clone()));
        }
    }
    Ok(held
        .iter()
        .copied()
        .filter(|i| wanted.contains(&i.name))
        // ALLOC: one pointer for each index.
        .collect())
}

/// An index as the native restore takes it, refused when a create would
/// refuse its mapping or settings.
fn native(
    meta: &StockIndexMetadata,
    rules: &StockRules<'_>,
) -> Result<StockNative, StockPlanError> {
    let native = meta
        .native(rules.keeps)
        .map_err(|r| StockPlanError::Unsupported(r.to_string()))?; // ALLOC: error path; one message.
    if let Some(mapping) = &native.mapping
        && !(rules.mapping_ok)(mapping)
    {
        return Err(StockPlanError::Unsupported(
            "the index has a mapping this node cannot index".to_owned(), // ALLOC: error path; one message.
        ));
    }
    Ok(native)
}
