//! `GET /_snapshot`, `/_snapshot/{repo}` and `/_snapshot/{repo}/{name}`: the
//! repositories and snapshots a deployment holds, read-only.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/rest/SnapshotHandler.java`.
//! The decisions and the renderings are `oss_coordinator::snapshot_admin`'s,
//! the reads `oss_meta::SnapshotReader`'s; a write route answers 501.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_coordinator::snapshot_admin::{
    check_name, is_selector, render_repositories, render_snapshot, repository_missing,
    snapshot_missing, unsupported_selector, unsupported_write,
};
use oss_format::{MAX_REPOSITORY_RECORD_BYTES, MAX_SNAPSHOT_RECORD_BYTES};
use oss_meta::{SnapshotReadError, SnapshotReader};
use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router};

use crate::route_util::{refuse, refused};

/// The query parameters a snapshot read accepts and has nothing to do for.
const PARAMS: &[&str] = &[
    "master_timeout",
    "cluster_manager_timeout",
    "ignore_unavailable",
    "verbose",
    "local",
];

/// Serves the snapshot read routes over the store.
pub struct SnapshotService {
    reader: SnapshotReader,
}

fn failed(e: &SnapshotReadError) -> RestError {
    match e {
        SnapshotReadError::Blob(_) => {
            // ALLOC: error path; the failure's words.
            refuse(503, "metadata_unavailable", e.to_string())
        }
        SnapshotReadError::TooManyRepositories => {
            // ALLOC: error path; the failure's words.
            refuse(400, "too_many_repositories", e.to_string())
        }
        // ALLOC: error path; the failure's words.
        _ => refuse(500, "snapshot_unreadable", e.to_string()),
    }
}

impl SnapshotService {
    /// A service over `store` under `base`.
    pub fn new(store: Arc<dyn BlobStore>, base: &str) -> Self {
        Self {
            reader: SnapshotReader::new(store, base),
        }
    }

    /// `GET /_snapshot[/{repo}[/{name}]]`.
    fn read(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let repo = req.path_param("repo").map(|r| r.into_owned());
        let name = req.path_param("name").map(|n| n.into_owned());
        if let Some(repo) = repo.as_deref() {
            if is_selector(repo) {
                return Err(refused(unsupported_selector(repo)));
            }
            check_name(repo, "repository").map_err(refused)?;
        }
        if let Some(name) = name.as_deref() {
            if is_selector(name) {
                return Err(refused(unsupported_selector(name)));
            }
            check_name(name, "snapshot").map_err(refused)?;
        }
        // A repository record is read through the smaller buffer.
        let size = if name.is_some() {
            MAX_SNAPSHOT_RECORD_BYTES
        } else {
            MAX_REPOSITORY_RECORD_BYTES
        };
        req.body().reserve(size)?;
        // ALLOC: one buffer a request, `size` bytes, reserved above.
        let mut buf = vec![0u8; size];
        let body = match (repo.as_deref(), name.as_deref()) {
            (None, _) => {
                let records = self.reader.repositories(&mut buf).map_err(|e| failed(&e))?;
                render_repositories(&records)
            }
            (Some(repo), None) => {
                let record = self
                    .reader
                    .repository(repo, &mut buf)
                    .map_err(|e| failed(&e))?
                    .ok_or_else(|| refused(repository_missing(repo)))?;
                render_repositories(&[record])
            }
            (Some(repo), Some(name)) => {
                let read = self
                    .reader
                    .snapshot(repo, name, &mut buf)
                    .map_err(|e| failed(&e))?
                    .ok_or_else(|| refused(snapshot_missing(repo, name)))?;
                render_snapshot(&read.summary)
            }
        };
        resp.send(200, JSON, body.as_bytes())
    }
}

/// The snapshot routes: reads served, writes refused with 501.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: &Arc<SnapshotService>) -> Result<Router, RouteError> {
    let mut router = router;
    for path in [
        "/_snapshot",
        "/_snapshot/{repo}",
        "/_snapshot/{repo}/{name}",
    ] {
        let service = Arc::clone(service);
        router = router.route(
            Method::Get,
            path,
            PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| service.read(req, resp),
        )?;
    }
    for path in ["/_snapshot/{repo}", "/_snapshot/{repo}/{name}"] {
        for method in [Method::Put, Method::Post, Method::Delete] {
            router = router.route(
                method,
                path,
                PARAMS,
                |_req: &mut Request<'_>, _resp: &mut Response<'_>| {
                    Err(refused(unsupported_write()))
                },
            )?;
        }
    }
    Ok(router)
}
