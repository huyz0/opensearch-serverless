//! `PUT`/`POST /_snapshot/{repo}/{name}`: takes a shallow snapshot.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/snapshot/SnapshotOperations.java`
//! (`capture`) and `SnapshotHandler`'s create branch (ADR 0042). The request
//! and the answers are `oss_coordinator::snapshot_take_admin`'s, the steps
//! against the store `oss_meta::SnapshotTaker`'s; this is the order they run
//! in, and the cleanup when one fails.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_coordinator::repository_admin::ACKNOWLEDGED as DELETED;
use oss_coordinator::snapshot_admin::{
    check_name, is_selector, render_snapshot, repository_missing, snapshot_missing,
    unsupported_selector,
};
use oss_coordinator::snapshot_take_admin::{
    ACCEPTED, already_exists, check_shards, conflict, index_not_found, is_alias, name_too_long,
    nothing_published, record_too_large, request, system_index, unsupported_layout,
    unsupported_storage,
};
use oss_coordinator::system_indices::SystemIndices;
use oss_format::{
    IndexCapture, MAX_REPOSITORY_RECORD_BYTES, MAX_SNAPSHOT_RECORD_BYTES, SnapshotDraft,
    SnapshotSummary,
};
use oss_meta::{
    CaptureError, DescribedIndex, DescriptorStore, MetaError, PitClock, SnapshotReadError,
    SnapshotReader, SnapshotTaker, TakeError, new_id,
};
use oss_rest::{JSON, Request, Response, RestError};

use crate::route_util::{flag, read_body, refuse, refused};

/// What a take reads and writes, under one base.
pub struct SnapshotTakeService {
    pub(crate) reader: SnapshotReader,
    pub(crate) taker: SnapshotTaker,
    pub(crate) descriptors: DescriptorStore,
    pub(crate) clock: PitClock,
    pub(crate) system: Arc<SystemIndices>,
    pub(crate) export: crate::stock_export_route::StockExportSettings,
}

pub(crate) fn unavailable(e: &dyn std::fmt::Display) -> RestError {
    // ALLOC: error path; the failure's words.
    refuse(503, "metadata_unavailable", e.to_string())
}

pub(crate) fn taken(e: TakeError, repo: &str, name: &str) -> RestError {
    match e {
        TakeError::AlreadyExists => refused(already_exists(repo, name)),
        TakeError::KeyTooLong => refused(name_too_long(repo, name)),
        TakeError::RecordTooLarge(n) => refused(record_too_large(n)),
        TakeError::Conflict(why) => refused(conflict(why)),
        TakeError::Blob(ref b) => unavailable(b),
        // ALLOC: error path; the failure's words.
        other => refuse(500, "snapshot_failure", other.to_string()),
    }
}

pub(crate) fn captured(e: CaptureError, index: &str) -> RestError {
    match e {
        CaptureError::NoSuchIndex => refused(index_not_found(index)),
        CaptureError::IsAlias => refused(is_alias(index)),
        CaptureError::Restoring => refuse(503, "cluster_block_exception", e.to_string()),
        CaptureError::Unsupported(_) => refused(unsupported_layout(index)),
        CaptureError::NothingPublished(shard) => refused(nothing_published(index, shard)),
        CaptureError::Replaced => refused(conflict("the index changed while it was captured")),
        CaptureError::Meta(MetaError::Blob(ref b)) => unavailable(b),
        // ALLOC: error path; the failure's words.
        CaptureError::Meta(other) => refuse(500, "snapshot_failure", other.to_string()),
    }
}

impl SnapshotTakeService {
    /// A service over `store` under `base`, resolving indices through
    /// `descriptors` and stamping a take from `clock`.
    pub fn new(
        store: Arc<dyn BlobStore>,
        base: &str,
        descriptors: DescriptorStore,
        clock: PitClock,
    ) -> Self {
        Self {
            reader: SnapshotReader::new(Arc::clone(&store), base),
            taker: SnapshotTaker::new(store, base),
            descriptors,
            clock,
            system: Arc::new(SystemIndices::none()),
            export: crate::stock_export_route::StockExportSettings::off(),
        }
    }

    /// Lets a take into a stock repository export as `export` says.
    #[must_use]
    pub fn with_stock_export(
        mut self,
        export: crate::stock_export_route::StockExportSettings,
    ) -> Self {
        self.export = export;
        self
    }

    /// Refuses a take of any index `system` names, 403.
    #[must_use]
    pub fn with_system_indices(mut self, system: Arc<SystemIndices>) -> Self {
        self.system = system;
        self
    }

    /// The route's handler.
    pub(crate) fn take(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let repo = req.path_param("repo").map(|r| r.into_owned());
        let name = req.path_param("name").map(|n| n.into_owned());
        let (Some(repo), Some(name)) = (repo, name) else {
            return Err(RestError::illegal_argument(
                "a repository and name are required",
            ));
        };
        let wait = flag(req, "wait_for_completion", false);
        let body = read_body(
            req,
            MAX_REPOSITORY_RECORD_BYTES,
            "a snapshot request is larger than 1 MiB",
        )?;
        let asked = request(&repo, &name, Some(&body)).map_err(refused)?;
        req.body().reserve(MAX_SNAPSHOT_RECORD_BYTES)?;
        // ALLOC: one buffer a request, MAX_SNAPSHOT_RECORD_BYTES, reserved above.
        let mut value = vec![0u8; MAX_SNAPSHOT_RECORD_BYTES];
        match self
            .reader
            .repository(&repo, &mut value)
            .map_err(|e| read_failed(&e))?
        {
            None => return Err(refused(repository_missing(&repo))),
            Some(record) if record.is_stock() => {
                return self
                    .export_stock(req, resp, &repo, &name, &asked, wait, &record, &mut value);
            }
            Some(record) if !record.shallow_by_default() => {
                return Err(refused(unsupported_storage(&repo)));
            }
            Some(_) => {}
        }
        let described = self.describe(&asked, &mut value)?;
        let start = i64::try_from((self.clock)()).unwrap_or(i64::MAX);
        let generation = new_id().map_err(|e| unavailable(&e))?;
        let uuid = new_id().map_err(|e| unavailable(&e))?;
        let draft = |indices: Vec<IndexCapture>, end: i64| SnapshotDraft {
            // ALLOC: the names, once a take.
            repo: repo.clone(),
            name: name.clone(),
            uuid: uuid.clone(),
            storage_generation: Some(generation.clone()),
            shallow: true,
            start_millis: start,
            end_millis: end,
            indices,
        };
        // ALLOC: one entry an index, at most MAX_TAKE_INDICES.
        let reserved = described
            .iter()
            .map(|d| d.capture(Vec::new(), true))
            .collect();
        let reservation = self
            .taker
            .reserve(&draft(reserved, start), &mut value)
            .map_err(|e| taken(e, &repo, &name))?;
        // ALLOC: one entry an index, at most MAX_TAKE_INDICES.
        let protective = described
            .iter()
            .map(|d| d.capture(Vec::new(), false))
            .collect();
        let run = || -> Result<SnapshotDraft, RestError> {
            self.taker
                .publish_marker(&reservation, &draft(protective, start), &mut value)
                .map_err(|e| taken(e, &repo, &name))?;
            for index in &described {
                self.taker
                    .verify(&self.descriptors, index, &mut value)
                    .map_err(|e| captured(e, &index.name))?;
            }
            let end = i64::try_from((self.clock)()).unwrap_or(i64::MAX);
            let finished = draft(
                self.capture_all(&described, asked.partial, &mut value)?,
                end,
            );
            self.taker
                .finish(&reservation, &finished, &mut value)
                .map_err(|e| taken(e, &repo, &name))?;
            Ok(finished)
        };
        let finished = match run() {
            Ok(finished) => finished,
            Err(failure) => {
                // The failure being reported is the first.
                let _ = self.taker.abort(&reservation, &mut value);
                return Err(failure);
            }
        };
        if !wait {
            return resp.send(200, JSON, ACCEPTED.as_bytes());
        }
        let len = finished
            .write(&mut value)
            .map_err(|e| refuse(500, "snapshot_failure", e.to_string()))?;
        let summary = SnapshotSummary::parse(value.get(..len).unwrap_or_default())
            .map_err(|e| refuse(500, "snapshot_failure", e.to_string()))?;
        resp.send(200, JSON, render_snapshot(&summary).as_bytes())
    }

    /// `DELETE /_snapshot/{repo}/{name}`: releases the snapshot and removes
    /// its marker; a snapshot that copied its data is refused.
    pub(crate) fn delete(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let repo = req.path_param("repo").map(|r| r.into_owned());
        let name = req.path_param("name").map(|n| n.into_owned());
        let (Some(repo), Some(name)) = (repo, name) else {
            return Err(RestError::illegal_argument(
                "a repository and name are required",
            ));
        };
        for (value, what) in [(&repo, "repository"), (&name, "snapshot")] {
            if is_selector(value) {
                return Err(refused(unsupported_selector(value)));
            }
            check_name(value, what).map_err(refused)?;
        }
        req.body().reserve(MAX_SNAPSHOT_RECORD_BYTES)?;
        // ALLOC: one buffer a request, MAX_SNAPSHOT_RECORD_BYTES, reserved above.
        let mut value = vec![0u8; MAX_SNAPSHOT_RECORD_BYTES];
        match self.taker.remove(&repo, &name, &mut value) {
            Ok(true) => resp.send(200, JSON, DELETED.as_bytes()),
            Ok(false) => {
                // A snapshot only a legacy record holds is not ours to delete.
                let legacy = self
                    .reader
                    .snapshot(&repo, &name, &mut value)
                    .map_err(|e| read_failed(&e))?;
                if legacy.is_some() {
                    return Err(refuse(
                        501,
                        "unsupported_operation",
                        // ALLOC: error path; one message.
                        format!(
                            "[{repo}/{name}] is a legacy snapshot, which this node does not delete"
                        ),
                    ));
                }
                Err(refused(snapshot_missing(&repo, &name)))
            }
            Err(TakeError::NotShallow) => Err(refuse(
                501,
                "unsupported_operation",
                // ALLOC: error path; one message.
                format!("[{repo}/{name}] copied its data, which this node does not delete"),
            )),
            Err(e) => Err(taken(e, &repo, &name)),
        }
    }

    /// Resolves each named index, skipping a missing one with
    /// `ignore_unavailable`, and holds the take to its bounds.
    pub(crate) fn describe(
        &self,
        asked: &oss_coordinator::snapshot_take_admin::TakeRequest,
        buf: &mut [u8],
    ) -> Result<Vec<DescribedIndex>, RestError> {
        // ALLOC: one entry an index, at most MAX_TAKE_INDICES.
        let mut described = Vec::new();
        let mut shards = 0usize;
        for index in &asked.indices {
            if self.system.contains(index) {
                return Err(refused(system_index(index)));
            }
            match self.taker.describe(&self.descriptors, index, buf) {
                Ok(d) => {
                    shards = shards.saturating_add(d.shard_count());
                    check_shards(shards).map_err(refused)?;
                    described.push(d);
                }
                Err(CaptureError::NoSuchIndex) if asked.ignore_unavailable => {}
                Err(e) => return Err(captured(e, index)),
            }
        }
        if described.is_empty() {
            let first = asked.indices.first().map_or("", String::as_str);
            return Err(refused(index_not_found(first)));
        }
        Ok(described)
    }

    /// Reads every shard's commit; an index with a shard that published
    /// nothing is skipped with `partial`, else refused. Nothing captured at
    /// all is refused.
    fn capture_all(
        &self,
        described: &[DescribedIndex],
        partial: bool,
        buf: &mut [u8],
    ) -> Result<Vec<IndexCapture>, RestError> {
        // ALLOC: one entry an index, at most MAX_TAKE_INDICES.
        let mut out = Vec::new();
        'indices: for index in described {
            // ALLOC: one entry a shard, at most the take's shard bound.
            let mut shards = Vec::new();
            for &shard in index.units() {
                match self.taker.capture_shard(index, shard, buf) {
                    Ok(s) => shards.push(s),
                    Err(CaptureError::NothingPublished(_)) if partial => continue 'indices,
                    Err(e) => return Err(captured(e, &index.name)),
                }
            }
            out.push(index.capture(shards, true));
        }
        if out.is_empty() {
            let first = described.first().map_or("", |d| d.name.as_str());
            return Err(refused(nothing_published(first, 0)));
        }
        Ok(out)
    }
}

fn read_failed(e: &SnapshotReadError) -> RestError {
    match e {
        SnapshotReadError::Blob(_) => unavailable(e),
        // ALLOC: error path; the failure's words.
        _ => refuse(500, "snapshot_unreadable", e.to_string()),
    }
}
