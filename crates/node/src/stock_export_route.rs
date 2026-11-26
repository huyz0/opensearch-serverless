//! A take into a stock OpenSearch repository: the target store under the
//! export allow-lists, the refusals before any write, then the copy and the
//! publish `oss_meta::stock_export` and `stock_export_take` hold.
//!
//! Reached from `SnapshotTakeService::take` when the repository is
//! registered `format: "stock"`; ADR 0062.

use std::sync::Arc;

use oss_blob::{BlobError, BlobStore, ExportStore, FsStore, S3Config, S3Store, SourceRoots};
use oss_coordinator::snapshot_admin::render_snapshot;
use oss_coordinator::snapshot_take_admin::{ACCEPTED, TakeRequest, already_exists, conflict};
use oss_format::{
    ClusterSettings, IndexCapture, MAX_SNAPSHOT_RECORD_BYTES, RepositoryRecord, Setting,
    ShardCapture, ShardRef, SnapshotDraft, SnapshotSummary,
};
use oss_meta::{
    CopyBuffers, DescribedIndex, ExportBase, ExportError, ExportIndex, ExportShard, ExportSnapshot,
    ExportTarget, MAX_INDEX_N_BYTES, new_id, open_export_base, publish_snapshot,
};
use oss_rest::{JSON, Request, Response, RestError};

use crate::route_util::{refuse, refused};
use crate::snapshot_take_route::{SnapshotTakeService, captured, unavailable};

/// What a take into a stock repository may write, set by the node's
/// configuration.
pub struct StockExportSettings {
    /// `serverless.snapshots.stock.export.enabled`.
    pub enabled: bool,
    /// The directories and buckets a target may sit under.
    pub roots: SourceRoots,
    /// How an `s3` target is reached, and this deployment's own bucket and
    /// key prefix; none answers 501.
    pub s3: Option<(crate::stock_s3::SourceS3, String)>,
    /// The bytes of one ranged read; two buffers of it are held.
    pub chunk_bytes: usize,
}

impl StockExportSettings {
    /// Off: a take into a stock repository answers 501 and writes nothing.
    #[must_use]
    pub fn off() -> Self {
        Self {
            enabled: false,
            roots: SourceRoots::parse("", ""),
            s3: None,
            chunk_bytes: 1 << 20,
        }
    }
}

/// The repository's setting `key`, or empty.
fn setting<'a>(record: &'a RepositoryRecord, key: &str) -> &'a str {
    record
        .settings
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
        .unwrap_or_default()
}

/// The longest snapshot name, as the shell's.
const MAX_NAME_BYTES: usize = 128;

/// The most one stored object may hold (S3's single-put limit).
const MAX_OBJECT_BYTES: u64 = 5 << 30;

fn unsupported(reason: impl Into<String>) -> RestError {
    refuse(501, "unsupported_operation", reason)
}

/// The snapshot name stock accepts: letters, digits, dot, dash, underscore.
fn name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_BYTES
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

/// The native settings as the pairs stock carries; a list is refused.
fn settings_of(index: &DescribedIndex) -> Result<Vec<(String, String)>, RestError> {
    let Some(bytes) = &index.settings else {
        return Ok(Vec::new());
    };
    let bad = || {
        unsupported(format!(
            "the settings of [{}] cannot be written to a stock repository",
            index.name
        ))
    };
    let parsed = ClusterSettings::parse(bytes).map_err(|_| bad())?;
    parsed
        .settings()
        .iter()
        .map(|(k, v)| match v {
            Setting::Value(v) => Ok((k.clone(), v.clone())), // ALLOC: one pair a setting.
            Setting::List(_) => Err(bad()),
        })
        .collect()
}

/// A mapping that is one JSON object, as stock's `_doc` entry is.
fn mapping_ok(mapping: &str) -> bool {
    matches!(
        oss_doc::mapping::json::parse(mapping.as_bytes()),
        Ok(oss_doc::mapping::json::Json::Obj(_))
    )
}

/// A copy's failure as the request's answer.
fn export_failed(e: ExportError, repo: &str, name: &str) -> RestError {
    match e {
        ExportError::NameTaken => refused(already_exists(repo, name)),
        ExportError::Conflict | ExportError::Blob(BlobError::NotFound) => refused(conflict(
            "the repository or a source file changed while the snapshot was taken",
        )),
        ExportError::Blob(BlobError::Busy) => refuse(429, "too_many_requests", e.to_string()),
        ExportError::Blob(ref b) => unavailable(b),
        ExportError::Unsupported(why) => unsupported(why),
        other => refuse(500, "snapshot_failure", other.to_string()),
    }
}

/// The commit's generation, from its `segments_N` (`N` in base 36).
fn commit_generation(files: &[oss_meta::ExportedFile]) -> u64 {
    files
        .iter()
        .filter_map(|f| f.physical_name.strip_prefix("segments_"))
        .filter_map(|n| u64::from_str_radix(n, 36).ok())
        .max()
        .unwrap_or(0)
}

impl SnapshotTakeService {
    /// The take of `name` into `repo`, a stock repository.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn export_stock(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
        repo: &str,
        name: &str,
        asked: &TakeRequest,
        wait: bool,
        record: &RepositoryRecord,
        value: &mut [u8],
    ) -> Result<(), RestError> {
        if !name_ok(name) {
            return Err(refuse(
                400,
                "invalid_snapshot_name_exception",
                format!("Invalid snapshot name [{name}], use letters, digits, '.', '-' and '_'"),
            ));
        }
        if asked.include_global_state {
            return Err(unsupported(
                "include_global_state is not supported: a take into a stock repository captures no cluster state",
            ));
        }
        if asked.partial {
            return Err(unsupported(
                "partial is not supported by a take into a stock repository",
            ));
        }
        if !self.export.enabled {
            return Err(unsupported(
                "taking into a stock repository is not enabled on this node (serverless.snapshots.stock.export.enabled)",
            ));
        }
        if setting(record, "readonly") == "true" {
            return Err(refuse(
                400,
                "repository_exception",
                format!("[{repo}] is read-only"),
            ));
        }
        let described = self.describe(asked, value)?;
        let mut inputs = Vec::with_capacity(described.len()); // ALLOC: one entry an index.
        for index in &described {
            if index.bucketed() {
                return Err(unsupported(format!(
                    "[{}] lives in shared packs, which a stock repository cannot hold",
                    index.name
                )));
            }
            let settings = settings_of(index)?;
            if let Some(m) = &index.mapping
                && !mapping_ok(m)
            {
                return Err(unsupported(format!(
                    "the mapping of [{}] cannot be written to a stock repository",
                    index.name
                )));
            }
            inputs.push(settings);
        }
        let captures = self.capture_indices(&described, value)?;
        let (store, prefix) = self.target_store(record)?;
        let chunk = self.export.chunk_bytes;
        req.body()
            .reserve(chunk.saturating_mul(2).saturating_add(MAX_INDEX_N_BYTES))?;
        // ALLOC: two buffers a request, `chunk` each, reserved above.
        let (mut read, mut write) = (vec![0u8; chunk], vec![0u8; chunk]);
        let target = ExportTarget {
            store: &store,
            prefix: &prefix,
        };
        let base = open_export_base(&target).map_err(|e| export_failed(e, repo, name))?;
        let start = i64::try_from((self.clock)()).unwrap_or(i64::MAX);
        let uuid = new_id().map_err(|e| unavailable(&e))?;
        let exported = self.copy_indices(
            &described,
            &captures,
            &base,
            &target,
            repo,
            (&mut read, &mut write),
        )?;
        for index in &described {
            self.taker
                .verify(&self.descriptors, index, value)
                .map_err(|e| captured(e, &index.name))?;
        }
        let end = i64::try_from((self.clock)()).unwrap_or(i64::MAX);
        let indices: Vec<ExportIndex<'_>> = described // ALLOC: one entry an index.
            .iter()
            .zip(&exported)
            .zip(&inputs)
            .map(|((d, (id, shards)), settings)| ExportIndex {
                name: &d.name,
                id,
                uuid: &d.uuid,
                creation_date_ms: u64::try_from(d.created_at_millis).unwrap_or(0),
                mapping: d.mapping.as_deref(),
                settings,
                shards,
            })
            .collect();
        let snapshot = ExportSnapshot {
            name,
            uuid: &uuid,
            start_ms: u64::try_from(start).unwrap_or(0),
            end_ms: u64::try_from(end).unwrap_or(0),
            indices: &indices,
        };
        publish_snapshot(&target, base, &snapshot, &mut write)
            .map_err(|e| export_failed(e, repo, name))?;
        if !wait {
            return resp.send(200, JSON, ACCEPTED.as_bytes());
        }
        let finished = SnapshotDraft {
            repo: repo.to_owned(), // ALLOC: one name, once a take.
            name: name.to_owned(), // ALLOC: one name, once a take.
            uuid,
            storage_generation: None,
            shallow: false,
            start_millis: start,
            end_millis: end,
            indices: described
                .iter()
                .zip(captures)
                .map(|(d, shards)| d.capture(shards, true))
                .collect::<Vec<IndexCapture>>(), // ALLOC: one entry an index.
        };
        req.body().reserve(MAX_SNAPSHOT_RECORD_BYTES)?;
        // ALLOC: one buffer a request, MAX_SNAPSHOT_RECORD_BYTES, reserved above.
        let mut out = vec![0u8; MAX_SNAPSHOT_RECORD_BYTES];
        let len = finished
            .write(&mut out)
            .map_err(|e| refuse(500, "snapshot_failure", e.to_string()))?;
        let summary = SnapshotSummary::parse(out.get(..len).unwrap_or_default())
            .map_err(|e| refuse(500, "snapshot_failure", e.to_string()))?;
        resp.send(200, JSON, render_snapshot(&summary).as_bytes())
    }

    /// Reads every shard's commit, after checking each index is still the
    /// one described and no file is past the store's single-object limit:
    /// every refusal comes before the first write.
    fn capture_indices(
        &self,
        described: &[DescribedIndex],
        value: &mut [u8],
    ) -> Result<Vec<Vec<ShardCapture>>, RestError> {
        let mut all = Vec::with_capacity(described.len()); // ALLOC: one entry an index.
        for index in described {
            self.taker
                .verify(&self.descriptors, index, value)
                .map_err(|e| captured(e, &index.name))?;
            let mut taken = Vec::with_capacity(index.units().len()); // ALLOC: one entry a shard.
            for &shard in index.units() {
                let capture = self
                    .taker
                    .capture_shard(index, shard, value)
                    .map_err(|e| captured(e, &index.name))?;
                if capture.lengths.iter().any(|(_, n)| *n > MAX_OBJECT_BYTES) {
                    return Err(unsupported(format!(
                        "[{}] has a file above the {MAX_OBJECT_BYTES}-byte limit of one stored object",
                        index.name
                    )));
                }
                taken.push(capture);
            }
            all.push(taken);
        }
        Ok(all)
    }

    /// Copies every captured shard; each index's id and shards.
    fn copy_indices(
        &self,
        described: &[DescribedIndex],
        captures: &[Vec<ShardCapture>],
        base: &ExportBase,
        target: &ExportTarget<'_>,
        repo: &str,
        buffers: (&mut [u8], &mut [u8]),
    ) -> Result<Vec<(String, Vec<ExportShard>)>, RestError> {
        let (read, write) = buffers;
        let mut exported = Vec::with_capacity(described.len()); // ALLOC: one entry an index.
        for (index, taken) in described.iter().zip(captures) {
            let id = match base.index_id(&index.name) {
                Some(held) => held.to_owned(), // ALLOC: one id an index.
                None => new_id().map_err(|e| unavailable(&e))?,
            };
            let mut shards = Vec::with_capacity(taken.len()); // ALLOC: one entry a shard.
            for capture in taken {
                let files = self
                    .taker
                    .export_shard(
                        ShardRef {
                            index: &index.name,
                            uuid: &index.uuid,
                            shard: capture.shard,
                        },
                        capture,
                        target,
                        &id,
                        &mut CopyBuffers { read, write },
                    )
                    .map_err(|e| export_failed(e, repo, &index.name))?;
                shards.push(ExportShard {
                    index_version: commit_generation(&files),
                    files,
                });
            }
            exported.push((id, shards));
        }
        Ok(exported)
    }

    /// The target store and prefix `record` names, when they sit under an
    /// export root and clear this deployment's own bucket and prefix.
    fn target_store(&self, record: &RepositoryRecord) -> Result<(ExportStore, String), RestError> {
        // ALLOC: error path; one message.
        let bad = |e: &dyn std::fmt::Display| refuse(400, "repository_exception", e.to_string());
        let inner: Arc<dyn BlobStore>;
        let prefix;
        match record.repo_type.as_str() {
            "fs" => {
                let path = self
                    .export
                    .roots
                    .fs_location(setting(record, "location"))
                    .map_err(|e| bad(&e))?;
                inner = Arc::new(FsStore::new(path).map_err(|e| bad(&e))?);
                prefix = String::new();
            }
            "s3" => {
                let Some((source, own_prefix)) = &self.export.s3 else {
                    return Err(unsupported(
                        "this node's store is not S3, so it writes no stock s3 repository",
                    ));
                };
                let bucket = setting(record, "bucket");
                prefix = self
                    .export
                    .roots
                    .s3_prefix(
                        bucket,
                        setting(record, "base_path"),
                        &source.own_bucket,
                        own_prefix,
                    )
                    .map_err(|e| bad(&e))?;
                let config = S3Config::new(
                    &source.endpoint,
                    bucket,
                    &source.region,
                    source.credentials.clone(),
                );
                inner = Arc::new(S3Store::new(config).map_err(|e| bad(&e))?);
            }
            _ => return Err(unsupported("a stock repository is type fs or s3")),
        }
        let store = ExportStore::new(inner, &prefix);
        let closed = if prefix.is_empty() || prefix.ends_with('/') {
            prefix
        } else {
            format!("{prefix}/")
        };
        Ok((store, closed))
    }
}
