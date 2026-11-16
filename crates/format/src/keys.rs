//! Blob keys, derived from names and records already held, never listed.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/metadata/RegisterMap.java`
//! (`indices`, `descriptorBlob`, `shardData`, `reclaim`), `store/SegmentPublisher.java`
//! (`MANIFEST`, the `t=<term>` container), `store/NodePackStore.java` (`ROOT`,
//! `epochDir`, `name`) and `store/FileLocation.java` (`containerPath`). Every
//! key is relative to the deployment's base, which is empty or ends in `/`:
//!
//! | key | what |
//! |---|---|
//! | `indices/<index>` | the index's descriptor register |
//! | `tombstones/<hour>/<index>` | a deleted index's marker, empty |
//! | `index-changes/stripe-<nn>` | a change stripe's register |
//! | `reclaim/<minute>/<index>#<uuid>` | a deleted index's reclaim intent |
//! | `segments/<index>#<uuid>#<shard>/manifest` | the shard's commit register |
//! | `segments/<index>#<uuid>#<shard>/t=<term>/<blob>` | a blob in a term container |
//! | `packs/<node>/e=<epoch:020>/<ordinal:020>` | a node pack |

use std::borrow::Cow;
use std::fmt::Write as _;

use crate::error::{FormatError, corrupt};
use crate::location::FileLocation;
use crate::manifest::CommitManifest;

const WHAT: &str = "blob key";

/// The name of a shard's commit register in its storage path.
pub const MANIFEST: &str = "manifest";

/// One shard's storage: the index's name and uuid, and the shard number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShardRef<'a> {
    /// The index's name.
    pub index: &'a str,
    /// The index's uuid, from its descriptor.
    pub uuid: &'a str,
    /// The shard.
    pub shard: u32,
}

/// Where one segment file's bytes are: a blob key and a range of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileExtent<'a> {
    /// The Lucene file name.
    pub name: Cow<'a, str>,
    /// The blob's key, relative to the deployment's base.
    pub key: String,
    /// Where the file starts in the blob.
    pub offset: u64,
    /// The file's length; `None` only for a file published as a blob of its
    /// own whose manifest records no length, which a reader learns with a
    /// HEAD of `key`.
    pub length: Option<u64>,
}

/// Where under a deployment's base the name registers are
/// (`RegisterMap.indices`): one key a name, below which nothing is stored.
pub const DESCRIPTORS_PREFIX: &str = "indices/";

/// Where under a deployment's base deletes file their markers
/// (`RegisterMap.tombstones`), one directory an hour.
pub const MARKERS_PREFIX: &str = "tombstones/";

/// Where under a deployment's base the change stripes are
/// (`MetadataPlane`'s `index-changes`).
pub const CHANGES_PREFIX: &str = "index-changes/";

/// The key of change stripe `stripe` (`index-changes/stripe-<nn>`).
///
/// # Errors
///
/// [`FormatError::Corrupt`] for a base that is not empty and does not end in
/// `/`, or a stripe of [`crate::STRIPES`] or more.
pub fn change_stripe_key(base: &str, stripe: usize) -> Result<String, FormatError> {
    check_base(base)?;
    if stripe >= crate::STRIPES {
        return Err(corrupt(WHAT, "no such change stripe"));
    }
    // ALLOC: one key, the length of its parts.
    let mut key = [base, CHANGES_PREFIX].concat();
    let _ = write!(key, "stripe-{stripe:02}");
    Ok(key)
}

/// The cluster config register's key under a base: the Java's
/// `RegisterMap.clusterConfig` container and its `settings` blob.
pub const CLUSTER_CONFIG_KEY: &str = "cluster/config/settings";

/// The key of the cluster config register (`cluster/config/settings`).
///
/// # Errors
///
/// [`FormatError::Corrupt`] for a base that is not empty and does not end in
/// `/`.
pub fn cluster_config_key(base: &str) -> Result<String, FormatError> {
    check_base(base)?;
    // ALLOC: one key, the length of its parts.
    Ok([base, CLUSTER_CONFIG_KEY].concat())
}

/// The key of an index's descriptor register.
///
/// # Errors
///
/// [`FormatError::Corrupt`] for a base that is not empty and does not end in
/// `/`, or an empty name or one holding `/`.
pub fn descriptor_key(base: &str, index: &str) -> Result<String, FormatError> {
    check_base(base)?;
    check_segment(index)?;
    // ALLOC: one key, the length of its parts.
    Ok([base, DESCRIPTORS_PREFIX, index].concat())
}

/// The key of the marker a delete files under its hour's bucket
/// (`tombstones/<bucket>/<index>`, `RegisterMap.tombstones`), from which a
/// sweep finds the tombstones to remove without listing every index.
///
/// # Errors
///
/// As [`descriptor_key`], and for a bucket that is empty or holds `/`.
pub fn tombstone_marker_key(base: &str, bucket: &str, index: &str) -> Result<String, FormatError> {
    check_base(base)?;
    check_segment(bucket)?;
    check_segment(index)?;
    // ALLOC: one key, the length of its parts.
    Ok([base, MARKERS_PREFIX, bucket, "/", index].concat())
}

/// Where under a deployment's base deletes file their reclaim intents
/// (`RegisterMap.reclaim`), one directory a minute they fall due in.
pub const RECLAIM_PREFIX: &str = "reclaim/";

/// The key of the reclaim intent the delete of incarnation `uuid` of
/// `index` files under the minute `bucket` (`reclaim/<bucket>/<index>#<uuid>`,
/// the Java's `ReclaimQueue.enqueue`).
///
/// # Errors
///
/// As [`descriptor_key`], for a bucket that is empty or holds `/`, and for
/// an index name or uuid holding `#`, which would make two intents collide.
pub fn reclaim_intent_key(
    base: &str,
    bucket: &str,
    index: &str,
    uuid: &str,
) -> Result<String, FormatError> {
    check_base(base)?;
    check_segment(bucket)?;
    for part in [index, uuid] {
        check_segment(part)?;
        if part.contains('#') {
            return Err(corrupt(WHAT, "an index name or uuid holds '#'"));
        }
    }
    // ALLOC: one key, the length of its parts.
    Ok([base, RECLAIM_PREFIX, bucket, "/", index, "#", uuid].concat())
}

impl ShardRef<'_> {
    /// The shard's storage path, ending in `/`.
    ///
    /// # Errors
    ///
    /// As [`descriptor_key`], and for an index name or uuid holding `#`,
    /// which would make two shards' paths collide.
    pub fn base_path(&self, base: &str) -> Result<String, FormatError> {
        check_base(base)?;
        for part in [self.index, self.uuid] {
            check_segment(part)?;
            if part.contains('#') {
                return Err(corrupt(WHAT, "an index name or uuid holds '#'"));
            }
        }
        // ALLOC: one path, the length of its parts plus ten digits.
        let mut path = String::with_capacity(
            base.len()
                .saturating_add(self.index.len())
                .saturating_add(self.uuid.len())
                .saturating_add(24),
        );
        let _ = write!(
            path,
            "{base}segments/{}#{}#{}/",
            self.index, self.uuid, self.shard
        );
        Ok(path)
    }

    /// The key of the shard's commit register.
    ///
    /// # Errors
    ///
    /// As [`ShardRef::base_path`].
    pub fn manifest_key(&self, base: &str) -> Result<String, FormatError> {
        let mut key = self.base_path(base)?;
        key.push_str(MANIFEST);
        Ok(key)
    }

    /// Where every file of `manifest` is, in name order.
    ///
    /// # Errors
    ///
    /// As [`ShardRef::base_path`], and for a node id or term container that
    /// would not make a key of its own (empty, `.` or `..`, or holding `/`
    /// where the format has none).
    pub fn resolve<'a>(
        &self,
        base: &str,
        manifest: &CommitManifest<'a>,
    ) -> Result<Vec<FileExtent<'a>>, FormatError> {
        let shard_base = self.base_path(base)?;
        // ALLOC: one entry a manifest file, at most manifest::MAX_FILES.
        let mut out = Vec::with_capacity(manifest.files().len());
        for file in manifest.files() {
            // ALLOC: one key a file, at most manifest::MAX_FILES of them.
            let mut key = String::new();
            let offset = match file.location {
                FileLocation::Whole { term_dir } => {
                    check_segment(term_dir)?;
                    check_segment(&file.name)?;
                    let _ = write!(key, "{shard_base}{term_dir}/{}", file.name);
                    0
                }
                FileLocation::Packed {
                    term_dir,
                    blob,
                    offset,
                    ..
                } => {
                    check_segment(term_dir)?;
                    check_relative(blob)?;
                    let _ = write!(key, "{shard_base}{term_dir}/{blob}");
                    offset
                }
                FileLocation::NodePacked {
                    node,
                    epoch,
                    ordinal,
                    offset,
                    ..
                } => {
                    key = node_pack_key(base, node, epoch, ordinal)?;
                    offset
                }
            };
            out.push(FileExtent {
                name: file.name.clone(),
                key,
                offset,
                length: file.length,
            });
        }
        Ok(out)
    }
}

/// The key of a node pack.
///
/// # Errors
///
/// As [`descriptor_key`], for the node id.
pub fn node_pack_key(
    base: &str,
    node: &str,
    epoch: u64,
    ordinal: u64,
) -> Result<String, FormatError> {
    check_base(base)?;
    check_segment(node)?;
    // ALLOC: one key, the length of its parts plus forty digits.
    let mut key = String::with_capacity(base.len().saturating_add(node.len()).saturating_add(52));
    let _ = write!(key, "{base}packs/{node}/e={epoch:020}/{ordinal:020}");
    Ok(key)
}

/// The prefix of every node's write log, under the cluster's base.
pub const NODE_LOG_PREFIX: &str = "nodelog/";

/// The key of one slot of a node's log: `NodeLogStore.epochPath` and `name`.
///
/// # Errors
///
/// [`FormatError::Corrupt`] for a base that is neither empty nor ends in
/// `/`, or a node id that is empty, holds `/` or is `.` or `..`.
pub fn node_log_key(
    base: &str,
    node: &str,
    epoch: u64,
    ordinal: u64,
) -> Result<String, FormatError> {
    check_base(base)?;
    check_segment(node)?;
    // ALLOC: one key, the length of its parts plus forty digits.
    let mut key = String::with_capacity(base.len().saturating_add(node.len()).saturating_add(54));
    let _ = write!(
        key,
        "{base}{NODE_LOG_PREFIX}{node}/e={epoch:020}/{ordinal:020}"
    );
    Ok(key)
}

/// The transport secret register's key under a base: the Java's
/// `RegisterMap.clusterConfig` container and its `transport-secret` blob.
pub const TRANSPORT_SECRET_KEY: &str = "cluster/config/transport-secret";

/// The key of the transport secret register
/// (`cluster/config/transport-secret`).
///
/// # Errors
///
/// [`FormatError::Corrupt`] for a base that is not empty and does not end in
/// `/`.
pub fn transport_secret_key(base: &str) -> Result<String, FormatError> {
    check_base(base)?;
    // ALLOC: one key, the length of its parts.
    Ok([base, TRANSPORT_SECRET_KEY].concat())
}

fn check_base(base: &str) -> Result<(), FormatError> {
    if base.is_empty() || base.ends_with('/') {
        Ok(())
    } else {
        Err(corrupt(WHAT, "a base is empty or ends in '/'"))
    }
}

/// One path segment: not empty, not a dot segment, no `/`.
fn check_segment(part: &str) -> Result<(), FormatError> {
    if part.is_empty() || part == "." || part == ".." || part.contains('/') {
        return Err(corrupt(
            WHAT,
            "a key part is empty, a dot segment, or holds '/'",
        ));
    }
    Ok(())
}

/// A blob name that may hold `/` (the Java takes everything between the
/// term container and `#`), each of whose segments is a plain one.
fn check_relative(blob: &str) -> Result<(), FormatError> {
    blob.split('/').try_for_each(check_segment)
}
