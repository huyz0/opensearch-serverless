//! A restore from a stock OpenSearch repository: the source store, the
//! pool reservations, the plan (`oss_meta::stock_plan`), then the same
//! create, copy and cleanup a native restore does.
//!
//! Reached from `SnapshotRestoreService::restore` when the native register
//! holds no snapshot of that name and the repository is registered
//! `format: "stock"`; ADR 0060.

use std::sync::Arc;

use oss_blob::{BlobStore, FsStore, ReadOnlyStore, S3Config, S3Store, SourceRoots};
use oss_coordinator::snapshot_admin::snapshot_missing;
use oss_coordinator::snapshot_restore_admin::{
    RestoreRequest, index_not_captured, plan, render_restored,
};
use oss_coordinator::snapshot_take_admin::{ACCEPTED, check_shards};
use oss_format::{FormatError, MAX_REPOSITORY_RECORD_BYTES, RepositoryRecord, RestoreIndex};
use oss_meta::{
    CopyBuffers, MAX_BLOB_BYTES, MAX_INDEX_N_BYTES, PlannedIndex, StockLimits, StockPlanError,
    StockReadError, StockRepo, StockRules, StockShardCopy, StockSource, new_id, stock_load,
    stock_plan,
};
use oss_rest::{JSON, Request, Response, RestError};

use crate::route_util::{refuse, refused};
use crate::snapshot_restore_route::{SnapshotRestoreService, restore_failed, unavailable};

/// The bytes a metadata or shard blob holds after inflating.
const INFLATED_BYTES: usize = 64 << 20;

/// The buffer a target's descriptor is read into.
const CHECK_BYTES: usize = 1 << 16;
/// The buffer a restored index's descriptor is written from.
const DESCRIPTOR_BYTES: usize = 1 << 20;

/// What a stock restore reads from, set by the node's configuration.
pub struct StockRestoreSettings {
    /// `serverless.snapshots.stock.enabled`.
    pub enabled: bool,
    /// The directories and buckets a source may sit under.
    pub roots: SourceRoots,
    /// The most blobs read at once in a round, at least 1.
    pub fanout: usize,
    /// How an `s3` source is reached; none answers 501.
    pub s3: Option<StockS3>,
}

/// This node's S3 endpoint, and where its own data sits in its bucket.
pub struct StockS3 {
    /// The endpoint, region, credentials and own bucket.
    pub source: crate::stock_s3::SourceS3,
    /// This deployment's key prefix in its own bucket.
    pub own_prefix: String,
}

impl StockRestoreSettings {
    /// Off: a stock repository's restore answers 501 and reads nothing.
    #[must_use]
    pub fn off() -> Self {
        Self {
            enabled: false,
            roots: SourceRoots::parse("", ""),
            fanout: 1,
            s3: None,
        }
    }
}

fn unsupported(reason: impl Into<String>) -> RestError {
    refuse(501, "unsupported_operation", reason)
}

/// A read failure as the request's answer.
fn read_failed(e: StockReadError) -> RestError {
    match e {
        StockReadError::Blob(b) => unavailable(&b),
        StockReadError::Format {
            error: FormatError::Unsupported { .. } | FormatError::TooLarge { .. },
            ..
        } => unsupported(e.to_string()),
        StockReadError::TooLarge { .. } => refuse(400, "snapshot_too_large", e.to_string()),
        other => refuse(400, "repository_exception", other.to_string()),
    }
}

fn plan_failed(e: StockPlanError, repo: &str, name: &str) -> RestError {
    match e {
        StockPlanError::UnknownSnapshot(_) => refused(snapshot_missing(repo, name)),
        StockPlanError::UnknownIndex(index) => refused(index_not_captured(&index)),
        StockPlanError::Unsupported(why) => unsupported(why),
        StockPlanError::TooLarge(_) => refuse(400, "snapshot_too_large", e.to_string()),
        StockPlanError::Corrupt(_) => refuse(400, "repository_exception", e.to_string()),
        StockPlanError::Read(read) => read_failed(read),
    }
}

impl SnapshotRestoreService {
    /// The restore of `repo`'s snapshot `name` when `repo` is a stock
    /// repository; a missing snapshot otherwise.
    pub(crate) fn restore_stock(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
        repo: &str,
        name: &str,
        asked: &RestoreRequest,
        wait: bool,
    ) -> Result<(), RestError> {
        req.body().reserve(MAX_REPOSITORY_RECORD_BYTES)?;
        // ALLOC: one buffer a request, MAX_REPOSITORY_RECORD_BYTES, reserved above.
        let mut buf = vec![0u8; MAX_REPOSITORY_RECORD_BYTES];
        let record = match self.reader.repository(repo, &mut buf) {
            Ok(Some(r)) if r.is_stock() => r,
            Ok(_) => return Err(refused(snapshot_missing(repo, name))),
            // ALLOC: error path; the failure's words.
            Err(e) => return Err(unavailable(&e)),
        };
        if !self.stock.enabled {
            return Err(unsupported(
                "restoring from a stock repository is not enabled on this node (serverless.snapshots.stock.enabled)",
            ));
        }
        let (source, prefix) = self.source_store(&record)?;
        let blobs = self
            .stock
            .fanout
            .max(1)
            .saturating_mul(MAX_BLOB_BYTES.saturating_add(INFLATED_BYTES));
        // `index-N` stays held while the blob reads run.
        req.body().reserve(
            blobs
                .saturating_add(MAX_INDEX_N_BYTES)
                .saturating_add(CHECK_BYTES),
        )?;
        let from = StockRepo::new(&source, &prefix);
        let data = stock_load(&from).map_err(read_failed)?;
        let snap = data
            .snapshot(name)
            .ok_or_else(|| refused(snapshot_missing(repo, name)))?;
        // ALLOC: one name an index, at most the repository's entry bound.
        let captured: Vec<String> = data.indices_of(snap).map(|i| i.name.clone()).collect();
        let targets = plan(asked, &captured).map_err(refused)?;
        let mut check = vec![0u8; CHECK_BYTES]; // ALLOC: one buffer a request, CHECK_BYTES, reserved above.
        for target in &targets {
            self.check_target(target, &mut check)?;
        }
        // ALLOC: one name an index.
        let wanted: Vec<String> = targets.iter().map(|t| t.source.clone()).collect();
        let keeps = |key: &str| crate::create_settings::is_known(key);
        let mapping_ok = |text: &str| oss_writer::mapping::Mapping::parse(text.as_bytes()).is_ok();
        let rules = StockRules {
            limits: StockLimits::default(),
            fanout: self.stock.fanout,
            keeps: &keeps,
            mapping_ok: &mapping_ok,
        };
        let planned = stock_plan(&from, &data, name, Some(&wanted), &rules)
            .map_err(|e| plan_failed(e, repo, name))?;
        let shards = planned
            .indices
            .iter()
            .fold(0usize, |n, i| n.saturating_add(i.shards.len()));
        check_shards(shards).map_err(refused)?;
        let copy = self.chunk_bytes;
        req.body().reserve(
            copy.saturating_mul(2)
                .saturating_add(self.deletes.remove_bytes())
                .saturating_add(DESCRIPTOR_BYTES),
        )?;
        // ALLOC: two buffers a request, `chunk_bytes` each, reserved above.
        let (mut read, mut write) = (vec![0u8; copy], vec![0u8; copy]);
        let mut descriptor = vec![0u8; DESCRIPTOR_BYTES]; // ALLOC: one buffer a request, DESCRIPTOR_BYTES, reserved above.
        let now = i64::try_from((self.clock)()).unwrap_or(i64::MAX);
        let from_store = StockSource {
            store: &source,
            prefix: &prefix,
        };
        // ALLOC: one entry an index, at most the plan's bound.
        let mut created: Vec<(String, String)> = Vec::new();
        let mut run = |created: &mut Vec<(String, String)>| -> Result<(), RestError> {
            for target in &targets {
                let Some(index) = planned.indices.iter().find(|i| i.name == target.source) else {
                    return Err(refused(index_not_captured(&target.source)));
                };
                let uuid = new_id().map_err(|e| unavailable(&e))?;
                created.push((target.target.clone(), uuid.clone()));
                // ALLOC: one entry a shard, at most the plan's bound.
                let copies: Vec<StockShardCopy<'_>> = index
                    .shards
                    .iter()
                    .zip(0u32..)
                    .map(|(s, shard)| StockShardCopy {
                        index_id: &index.id,
                        shard,
                        files: &s.files,
                    })
                    .collect();
                self.restorer
                    .restore_stock_index(
                        &self.descriptors,
                        &from_store,
                        &native_of(index),
                        &copies,
                        &target.target,
                        &uuid,
                        now,
                        &mut descriptor,
                        &mut CopyBuffers {
                            read: &mut read,
                            write: &mut write,
                        },
                    )
                    .map_err(|e| restore_failed(e, &target.target))?;
            }
            Ok(())
        };
        if let Err(failure) = run(&mut created) {
            // The failure being reported is the first.
            for (index, uuid) in &created {
                let _ = self.deletes.remove(index, uuid);
            }
            return Err(failure);
        }
        if !wait {
            return resp.send(200, JSON, ACCEPTED.as_bytes());
        }
        resp.send(
            200,
            JSON,
            render_restored(name, &targets, shards).as_bytes(),
        )
    }

    /// The store the repository's files are read from, read-only and under
    /// an operator-listed root, and the repository's key prefix in it.
    fn source_store(
        &self,
        record: &RepositoryRecord,
    ) -> Result<(ReadOnlyStore, String), RestError> {
        let setting = |key: &str| {
            record
                .settings
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str())
                .unwrap_or_default()
        };
        // ALLOC: error path; one message.
        let bad = |e: &dyn std::fmt::Display| refuse(400, "repository_exception", e.to_string());
        let store: Arc<dyn BlobStore>;
        let prefix;
        match record.repo_type.as_str() {
            "fs" => {
                let path = self
                    .stock
                    .roots
                    .fs_location(setting("location"))
                    .map_err(|e| bad(&e))?;
                store = Arc::new(FsStore::new(path).map_err(|e| bad(&e))?);
                prefix = String::new();
            }
            "s3" => {
                let Some(s3) = &self.stock.s3 else {
                    return Err(unsupported(
                        "this node's store is not S3, so it reads no stock s3 repository",
                    ));
                };
                let bucket = setting("bucket");
                prefix = self
                    .stock
                    .roots
                    .s3_prefix(
                        bucket,
                        setting("base_path"),
                        &s3.source.own_bucket,
                        &s3.own_prefix,
                    )
                    .map_err(|e| bad(&e))?;
                let config = S3Config::new(
                    &s3.source.endpoint,
                    bucket,
                    &s3.source.region,
                    s3.source.credentials.clone(),
                );
                store = Arc::new(S3Store::new(config).map_err(|e| bad(&e))?);
            }
            _ => return Err(unsupported("a stock repository is type fs or s3")),
        }
        Ok((ReadOnlyStore::new(store), prefix))
    }
}

/// The index as a native restore creates it: its shards copied separately.
fn native_of(index: &PlannedIndex) -> RestoreIndex {
    RestoreIndex {
        name: index.name.clone(), // ALLOC: one name an index.
        uuid: index.id.clone(),   // ALLOC: one id an index.
        number_of_shards: index.native.number_of_shards,
        mapping: index.native.mapping.clone(), // ALLOC: one mapping an index.
        settings: Some(index.native.settings.clone()), // ALLOC: one settings object an index.
        shards: Vec::new(),
    }
}
