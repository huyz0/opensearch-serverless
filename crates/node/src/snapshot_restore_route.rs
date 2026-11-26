//! `POST /_snapshot/{repo}/{name}/_restore`: restores indices of a shallow
//! snapshot by copying their files into new indices.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/snapshot/SnapshotOperations.java`
//! (`restore`) and `SnapshotHandler`'s restore branch (ADR 0045). The
//! request and the answers are `oss_coordinator::snapshot_restore_admin`'s,
//! the steps against the store `oss_meta::SnapshotRestorer`'s; this is the
//! order they run in, and the cleanup when one fails.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_coordinator::snapshot_restore_admin::{
    Target, in_progress, plan, plan_aliases, render_restored_with, request, restore_conflict,
    restore_disabled, target_exists, unsupported_content,
};
use oss_coordinator::snapshot_restore_settings::SettingsChange;
use oss_coordinator::snapshot_take_admin::{ACCEPTED, check_shards};
use oss_coordinator::system_indices::{SystemIndices, restore_refusal};
use oss_format::{MAX_REPOSITORY_RECORD_BYTES, MAX_SNAPSHOT_RECORD_BYTES, RestoreIndex};
use oss_meta::{
    CopyBuffers, DescriptorStore, MetaError, PitClock, RestoreError, RestoreRead,
    SnapshotReadError, SnapshotReader, SnapshotRestorer, new_id, validate_index_or_alias,
};
use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router};

use crate::index_delete::DeleteService;
use crate::restore_background::BackgroundRestores;
use crate::route_util::{flag, read_body, refuse, refused};
use crate::stock_restore_route::StockRestoreSettings;

/// The bytes one ranged read and one write of a restored file hold each.
pub const COPY_CHUNK_BYTES: usize = 4 << 20;

/// What a restore accepts besides the shell's hints.
const PARAMS: &[&str] = &[
    "master_timeout",
    "cluster_manager_timeout",
    "wait_for_completion",
];

/// What a restore reads and writes, under one base.
pub struct SnapshotRestoreService {
    enabled: bool,
    pub(crate) reader: SnapshotReader,
    pub(crate) restorer: SnapshotRestorer,
    pub(crate) descriptors: DescriptorStore,
    pub(crate) deletes: Arc<DeleteService>,
    pub(crate) clock: PitClock,
    pub(crate) chunk_bytes: usize,
    pub(crate) system: Arc<SystemIndices>,
    pub(crate) stock: StockRestoreSettings,
    background: Option<Arc<BackgroundRestores>>,
    pub(crate) alias_buffer_bytes: usize,
    pub(crate) alias_lease_millis: i64,
}

pub(crate) fn unavailable(e: &dyn std::fmt::Display) -> RestError {
    // ALLOC: error path; the failure's words.
    refuse(503, "metadata_unavailable", e.to_string())
}

pub(crate) fn restore_failed(e: RestoreError, index: &str) -> RestError {
    match e {
        RestoreError::Unsupported(why) => refused(unsupported_content(index, why)),
        RestoreError::Conflict { shard } => refused(restore_conflict(index, shard)),
        RestoreError::Meta(MetaError::AlreadyExists) => refused(target_exists(index)),
        RestoreError::Meta(MetaError::Blob(ref b)) | RestoreError::Blob(ref b) => unavailable(b),
        // ALLOC: error path; the failure's words.
        other => refuse(500, "restore_failure", other.to_string()),
    }
}

impl SnapshotRestoreService {
    /// A service over `store` under `base`. With `enabled` false every
    /// request is answered 501 before any request to the store. `deletes`
    /// undoes a restore that fails; each file moves through two buffers of
    /// `chunk_bytes`.
    pub fn new(
        store: Arc<dyn BlobStore>,
        base: &str,
        descriptors: DescriptorStore,
        deletes: Arc<DeleteService>,
        clock: PitClock,
        enabled: bool,
        chunk_bytes: usize,
    ) -> Self {
        Self {
            enabled,
            reader: SnapshotReader::new(Arc::clone(&store), base),
            restorer: SnapshotRestorer::new(store, base),
            descriptors,
            deletes,
            clock,
            chunk_bytes,
            system: Arc::new(SystemIndices::none()),
            stock: StockRestoreSettings::off(),
            background: None,
            alias_buffer_bytes: crate::snapshot_restore_aliases::DEFAULT_BUFFER_BYTES,
            alias_lease_millis: crate::snapshot_restore_aliases::DEFAULT_LEASE_MILLIS,
        }
    }

    /// Adds a restore's aliases through register buffers of `bytes`, each
    /// mutation's reservation lasting `lease_millis`.
    #[must_use]
    pub fn with_alias_buffers(mut self, bytes: usize, lease_millis: i64) -> Self {
        self.alias_buffer_bytes = bytes;
        self.alias_lease_millis = lease_millis;
        self
    }

    /// Serves restores from stock OpenSearch repositories as `stock` says.
    #[must_use]
    pub fn with_stock(mut self, stock: StockRestoreSettings) -> Self {
        self.stock = stock;
        self
    }

    /// Copies a native restore that does not wait on its own thread, as
    /// `restores` allows.
    #[must_use]
    pub fn with_background(mut self, restores: Arc<BackgroundRestores>) -> Self {
        self.background = Some(restores);
        self
    }

    /// Refuses a restore of any index `system` names, or into one, 403.
    #[must_use]
    pub fn with_system_indices(mut self, system: Arc<SystemIndices>) -> Self {
        self.system = system;
        self
    }

    /// The route's handler.
    pub(crate) fn restore(
        self: &Arc<Self>,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        if !self.enabled {
            return Err(refused(restore_disabled()));
        }
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
        let (record, summary) = match self
            .reader
            .for_restore(&repo, &name, &mut value)
            .map_err(|e| read_failed(&e))?
        {
            RestoreRead::Missing => {
                // A stock repository has no register: its snapshot is read
                // from the source repository instead.
                return self.restore_stock(req, resp, &repo, &name, &asked, wait);
            }
            RestoreRead::Legacy => {
                return Err(refuse(
                    501,
                    "unsupported_operation",
                    // ALLOC: error path; one message.
                    format!(
                        "[{repo}/{name}] is a legacy snapshot, which this node does not restore"
                    ),
                ));
            }
            RestoreRead::Held { record, summary } => (record, summary),
        };
        if !summary.shallow {
            return Err(refuse(
                501,
                "unsupported_operation",
                // ALLOC: error path; one message.
                format!("[{repo}/{name}] copied its data, which this node does not restore"),
            ));
        }
        if summary.in_progress() {
            return Err(refused(in_progress(&repo, &name)));
        }
        // ALLOC: one name an index, at most the take's bound.
        let captured: Vec<String> = summary.indices.iter().map(|i| i.name.clone()).collect();
        let targets = plan(&asked, &captured).map_err(refused)?;
        let sources = self.check(record, &targets, &asked.settings)?;
        let shards = sources
            .iter()
            .fold(0usize, |n, s| n.saturating_add(s.shards.len()));
        check_shards(shards).map_err(refused)?;
        // ALLOC: one slice an index, at most the take's bound.
        let recorded: Vec<&[oss_format::AliasCapture]> =
            sources.iter().map(|s| s.aliases.as_slice()).collect();
        let aliases = plan_aliases(&asked, &targets, &recorded).map_err(refused)?;
        if !aliases.is_empty() {
            req.body().reserve(self.alias_buffer_bytes)?;
            self.check_aliases(&aliases)?;
        }
        let cut = asked
            .include_aliases
            .then(|| crate::snapshot_restore_aliases::truncated(&targets, &sources));
        if !wait && let Some(restores) = self.background.as_ref().filter(|r| r.enabled()) {
            let aliases = (aliases, cut);
            return self.restore_in_background(
                restores,
                resp,
                &targets,
                sources,
                (record.len(), aliases),
            );
        }
        let copy = self.chunk_bytes;
        let alias_bytes = if asked.include_aliases {
            self.alias_bytes()
        } else {
            0
        };
        req.body().reserve(
            copy.saturating_mul(2)
                .saturating_add(self.deletes.remove_bytes())
                .saturating_add(alias_bytes),
        )?;
        // ALLOC: two buffers a request, `chunk_bytes` each, reserved above.
        let (mut read, mut write) = (vec![0u8; copy], vec![0u8; copy]);
        // ALLOC: one buffer a request, as large as the record and its settings.
        let mut descriptor = vec![0u8; record.len().saturating_add(4096)];
        let now = i64::try_from((self.clock)()).unwrap_or(i64::MAX);
        // Every index created so far, so a failure removes what it made.
        // ALLOC: one entry an index, at most the take's bound.
        let mut created: Vec<(String, String)> = Vec::new();
        let mut run = |created: &mut Vec<(String, String)>| -> Result<(), RestError> {
            for (target, source) in targets.iter().zip(&sources) {
                let uuid = new_id().map_err(|e| unavailable(&e))?;
                // Recorded first: a failure inside the create is cleaned too.
                created.push((target.target.clone(), uuid.clone()));
                self.restorer
                    .restore_index(
                        &self.descriptors,
                        source,
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
        // A restore that does not wait still adds its aliases before it
        // answers; a failure has no body to carry it, so it is logged.
        let report = cut.map(|truncated| self.add_aliases(&aliases, truncated, &|| false));
        if !wait {
            return resp.send(200, JSON, ACCEPTED.as_bytes());
        }
        resp.send(
            200,
            JSON,
            render_restored_with(&name, &targets, shards, report.as_ref()).as_bytes(),
        )
    }

    /// Refuses a target that names a system index, is not a valid name or
    /// names an index or alias already.
    pub(crate) fn check_target(&self, target: &Target, buf: &mut [u8]) -> Result<(), RestError> {
        for name in [&target.source, &target.target] {
            if self.system.contains(name) {
                return Err(refused(restore_refusal(name)));
            }
        }
        validate_index_or_alias(&target.target).map_err(|e| {
            // ALLOC: error path; the rule's words.
            refuse(400, "invalid_index_name_exception", e.to_string())
        })?;
        match self.descriptors.get(&target.target, buf) {
            Ok(None) => Ok(()),
            Ok(Some(_)) => Err(refused(target_exists(&target.target))),
            Err(MetaError::Blob(ref b)) => Err(unavailable(b)),
            // ALLOC: error path; the failure's words.
            Err(other) => Err(refuse(500, "restore_failure", other.to_string())),
        }
    }

    /// Reads every source's entry and refuses what cannot be restored, and
    /// every target that names an index or alias already, before anything
    /// is written.
    fn check(
        &self,
        record: &[u8],
        targets: &[Target],
        change: &SettingsChange,
    ) -> Result<Vec<RestoreIndex>, RestError> {
        // ALLOC: one entry an index, at most the take's bound.
        let mut sources = Vec::with_capacity(targets.len());
        // ALLOC: one buffer a request, the descriptor's size.
        let mut buf = vec![0u8; 1 << 16];
        for target in targets {
            let found = RestoreIndex::find(record, &target.source)
                .map_err(|e| refuse(500, "snapshot_unreadable", e.to_string()))?;
            let Some(mut source) = found else {
                return Err(refused(
                    oss_coordinator::snapshot_restore_admin::index_not_captured(&target.source),
                ));
            };
            let refusal = source
                .refusal()
                .map_err(|e| refuse(500, "snapshot_unreadable", e.to_string()))?;
            if let Some(why) = refusal {
                return Err(refused(unsupported_content(&target.source, why)));
            }
            if !source.failed_shards.is_empty() {
                return Err(refuse(
                    500,
                    "snapshot_restore_exception",
                    // ALLOC: error path; one message.
                    format!(
                        "index [{}] of the snapshot has shards {:?} with no captured commit; restoring a partial snapshot is not supported",
                        target.source, source.failed_shards
                    ),
                ));
            }
            self.check_target(target, &mut buf)?;
            source.settings = crate::restore_settings::apply(
                change,
                source.settings.take(),
                source.number_of_shards,
            )?;
            sources.push(source);
        }
        Ok(sources)
    }
}

fn read_failed(e: &SnapshotReadError) -> RestError {
    match e {
        SnapshotReadError::Blob(_) => unavailable(e),
        // ALLOC: error path; the failure's words.
        _ => refuse(500, "snapshot_unreadable", e.to_string()),
    }
}

/// The restore route added to `router`.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: Arc<SnapshotRestoreService>) -> Result<Router, RouteError> {
    router.route(
        Method::Post,
        "/_snapshot/{repo}/{name}/_restore",
        PARAMS,
        move |req: &mut Request<'_>, resp: &mut Response<'_>| service.restore(req, resp),
    )
}
