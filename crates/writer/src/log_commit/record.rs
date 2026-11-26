//! The commit key and the record a logged commit is.

use oss_format::ShardKey;

use crate::log::Position;

/// What follows a shard's uuid in the key a commit is logged under.
pub const KEY_SUFFIX: &str = "#commit";

/// The id of a commit's one record.
pub(super) const RECORD_ID: &str = "commit";

/// The key a shard's staged commit is logged under: the shard's number and
/// name, and its uuid with [`KEY_SUFFIX`].
pub fn commit_key(shard: &ShardKey) -> ShardKey {
    ShardKey::new(
        // ALLOC: one key or record a staged commit, each bounded by the manifest.
        format!("{}{KEY_SUFFIX}", shard.index_uuid),
        shard.shard,
        shard.index_name.clone(),
    )
}

/// Whether `shard` is a key a commit is logged under, not a shard.
pub fn is_commit_key(shard: &ShardKey) -> bool {
    shard.index_uuid.ends_with(KEY_SUFFIX)
}

/// The uuid of the index a uuid names, whether or not it carries the commit suffix.
pub fn shard_uuid(uuid: &str) -> &str {
    uuid.strip_suffix(KEY_SUFFIX).unwrap_or(uuid)
}

/// A staged commit as it is logged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitRecord {
    /// When the oldest pack the commit names was written, in milliseconds.
    pub oldest_pack_millis: i64,
    /// The log position the commit covers; `None` when it covers none.
    pub covered: Option<Position>,
    /// The manifest, as the register would hold it.
    pub manifest: String,
}

impl CommitRecord {
    /// The record's text: `<oldest pack millis> <epoch> <ordinal>\n<manifest>`,
    /// with `-1 -1` for no covered position.
    pub fn encode(&self) -> String {
        let (epoch, ordinal) = self.covered.map_or_else(
            // ALLOC: one key or record a staged commit, each bounded by the manifest.
            || ("-1".to_owned(), "-1".to_owned()),
            // ALLOC: one key or record a staged commit, each bounded by the manifest.
            |p| (p.epoch.to_string(), p.ordinal.to_string()),
        );
        // ALLOC: one key or record a staged commit, each bounded by the manifest.
        format!(
            "{} {epoch} {ordinal}\n{}",
            self.oldest_pack_millis, self.manifest
        )
    }

    /// The commit a record's text holds; `None` for text that is not one.
    pub fn parse(text: &str) -> Option<Self> {
        let (head, manifest) = text.split_once('\n')?;
        let mut parts = head.split(' ');
        let oldest_pack_millis = parts.next()?.parse().ok()?;
        let epoch: i64 = parts.next()?.parse().ok()?;
        let ordinal: i64 = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        let covered = match (u64::try_from(epoch), u64::try_from(ordinal)) {
            (Ok(epoch), Ok(ordinal)) => Some(Position { epoch, ordinal }),
            _ if epoch == -1 && ordinal == -1 => None,
            _ => return None,
        };
        Some(Self {
            oldest_pack_millis,
            covered,
            // ALLOC: one key or record a staged commit, each bounded by the manifest.
            manifest: manifest.to_owned(),
        })
    }
}
