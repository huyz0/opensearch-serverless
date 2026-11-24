//! `GET /_snapshot`, `/_snapshot/{repo}` and `/_snapshot/{repo}/{name}`: the
//! repositories and snapshots a deployment holds, read-only.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/rest/SnapshotHandler.java`.
//! The decisions and the renderings are `oss_coordinator::snapshot_admin`'s,
//! the reads `oss_meta::SnapshotReader`'s; a write route answers 501.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_coordinator::repository_admin::{
    ACKNOWLEDGED, check_not_reserved, deletion, in_use, registration, render_verify,
    too_many_snapshots_to_check, unreadable,
};
use oss_coordinator::snapshot_admin::{
    check_name, is_selector, render_repositories, render_snapshot, repository_missing,
    snapshot_missing, unsupported_selector, unsupported_write,
};
use oss_format::{MAX_REPOSITORY_RECORD_BYTES, MAX_SNAPSHOT_RECORD_BYTES};
use oss_meta::{
    PitClock, Removal, RepositoryWriteError, RepositoryWriter, SnapshotReadError, SnapshotReader,
};
use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router};

use crate::route_util::{read_body, refuse, refused};
use crate::snapshot_take_route::SnapshotTakeService;

/// The query parameters a snapshot read accepts and has nothing to do for.
const PARAMS: &[&str] = &[
    "master_timeout",
    "cluster_manager_timeout",
    "ignore_unavailable",
    "verbose",
    "local",
];

/// What a repository registration takes besides the shell's hints.
const WRITE_PARAMS: &[&str] = &[
    "master_timeout",
    "cluster_manager_timeout",
    "timeout",
    "verify",
    "ignore_unavailable",
    "verbose",
    "local",
];

/// What a take accepts besides the shell's hints.
const TAKE_PARAMS: &[&str] = &[
    "master_timeout",
    "cluster_manager_timeout",
    "wait_for_completion",
    "ignore_unavailable",
    "verbose",
    "local",
];

/// Serves the snapshot read routes over the store.
pub struct SnapshotService {
    reader: SnapshotReader,
    writer: RepositoryWriter,
    node_id: String,
    clock: PitClock,
}

fn write_failed(e: &RepositoryWriteError) -> RestError {
    match e {
        RepositoryWriteError::TooManyMarkers => {
            // ALLOC: error path; the failure's words.
            refuse(500, "too_many_snapshots_to_check", e.to_string())
        }
        RepositoryWriteError::Blob(_) => {
            // ALLOC: error path; the failure's words.
            refuse(503, "metadata_unavailable", e.to_string())
        }
        // ALLOC: error path; the failure's words.
        _ => refuse(500, "repository_failure", e.to_string()),
    }
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
    /// A service over `store` under `base`, answering as node `node_id` and
    /// stamping registrations from `clock`.
    pub fn new(store: Arc<dyn BlobStore>, base: &str, node_id: &str, clock: PitClock) -> Self {
        Self {
            reader: SnapshotReader::new(Arc::clone(&store), base),
            writer: RepositoryWriter::new(store, base),
            // ALLOC: one name, once a node.
            node_id: node_id.to_owned(),
            clock,
        }
    }

    /// `PUT` and `POST /_snapshot/{repo}`: registers the repository.
    fn register(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let name = req.path_param("repo").map(|r| r.into_owned());
        let name = name.ok_or_else(|| RestError::illegal_argument("a name is required"))?;
        check_not_reserved(&name).map_err(refused)?;
        check_name(&name, "repository").map_err(refused)?;
        let body = read_body(
            req,
            MAX_REPOSITORY_RECORD_BYTES,
            "a repository is larger than 1 MiB",
        )?;
        let now = i64::try_from((self.clock)()).unwrap_or(i64::MAX);
        let record = registration(&name, Some(&body), now).map_err(refused)?;
        req.body().reserve(MAX_REPOSITORY_RECORD_BYTES)?;
        // ALLOC: one buffer a request, MAX_REPOSITORY_RECORD_BYTES, reserved above.
        let mut buf = vec![0u8; MAX_REPOSITORY_RECORD_BYTES];
        self.writer
            .put(&record, &mut buf)
            .map_err(|e| write_failed(&e))?;
        resp.send(200, JSON, ACKNOWLEDGED.as_bytes())
    }

    /// `DELETE /_snapshot/{repos}`: removes each named repository.
    fn remove(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let names = req.path_param("repo").map(|r| r.into_owned());
        let names = names.ok_or_else(|| RestError::illegal_argument("a name is required"))?;
        let list = deletion(&names).map_err(refused)?;
        req.body().reserve(MAX_SNAPSHOT_RECORD_BYTES)?;
        // ALLOC: one buffer a request, MAX_SNAPSHOT_RECORD_BYTES, reserved above.
        let mut buf = vec![0u8; MAX_SNAPSHOT_RECORD_BYTES];
        for name in list {
            match self.writer.remove(name, &mut buf) {
                Ok(Removal::Removed) => {}
                Ok(Removal::Missing) => return Err(refused(repository_missing(name))),
                Ok(Removal::InUse) => return Err(refused(in_use(name))),
                Err(RepositoryWriteError::TooManyMarkers) => {
                    return Err(refused(too_many_snapshots_to_check(name)));
                }
                Err(e) => return Err(write_failed(&e)),
            }
        }
        resp.send(200, JSON, ACKNOWLEDGED.as_bytes())
    }

    /// `POST /_snapshot/{repo}/_verify`: this node, if the repository exists.
    fn verify(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let name = req.path_param("repo").map(|r| r.into_owned());
        let name = name.ok_or_else(|| RestError::illegal_argument("a name is required"))?;
        check_name(&name, "repository").map_err(refused)?;
        req.body().reserve(MAX_REPOSITORY_RECORD_BYTES)?;
        // ALLOC: one buffer a request, MAX_REPOSITORY_RECORD_BYTES, reserved above.
        let mut buf = vec![0u8; MAX_REPOSITORY_RECORD_BYTES];
        match self.reader.repository(&name, &mut buf) {
            Ok(Some(_)) => {}
            Ok(None) => return Err(refused(repository_missing(&name))),
            Err(SnapshotReadError::Blob(_)) => {
                return Err(refuse(
                    503,
                    "metadata_unavailable",
                    "the store is unavailable",
                ));
            }
            Err(_) => return Err(refused(unreadable(&name))),
        }
        let body = render_verify(&self.node_id, &self.node_id);
        resp.send(200, JSON, body.as_bytes())
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
pub fn routes(
    router: Router,
    service: &Arc<SnapshotService>,
    take: Option<&Arc<SnapshotTakeService>>,
) -> Result<Router, RouteError> {
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
    for method in [Method::Put, Method::Post] {
        let service = Arc::clone(service);
        router = router.route(
            method,
            "/_snapshot/{repo}",
            WRITE_PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| service.register(req, resp),
        )?;
    }
    let remover = Arc::clone(service);
    router = router.route(
        Method::Delete,
        "/_snapshot/{repo}",
        PARAMS,
        move |req: &mut Request<'_>, resp: &mut Response<'_>| remover.remove(req, resp),
    )?;
    let verifier = Arc::clone(service);
    router = router.route(
        Method::Post,
        "/_snapshot/{repo}/_verify",
        WRITE_PARAMS,
        move |req: &mut Request<'_>, resp: &mut Response<'_>| verifier.verify(req, resp),
    )?;
    // A snapshot is taken, and deleted, only with the take switch on.
    for method in [Method::Put, Method::Post] {
        router = match take {
            Some(take) => {
                let take = Arc::clone(take);
                router.route(
                    method,
                    "/_snapshot/{repo}/{name}",
                    TAKE_PARAMS,
                    move |req: &mut Request<'_>, resp: &mut Response<'_>| take.take(req, resp),
                )?
            }
            None => router.route(
                method,
                "/_snapshot/{repo}/{name}",
                PARAMS,
                |_req: &mut Request<'_>, _resp: &mut Response<'_>| {
                    Err(refused(unsupported_write()))
                },
            )?,
        };
    }
    router = match take {
        Some(take) => {
            let take = Arc::clone(take);
            router.route(
                Method::Delete,
                "/_snapshot/{repo}/{name}",
                PARAMS,
                move |req: &mut Request<'_>, resp: &mut Response<'_>| take.delete(req, resp),
            )?
        }
        None => router.route(
            Method::Delete,
            "/_snapshot/{repo}/{name}",
            PARAMS,
            |_req: &mut Request<'_>, _resp: &mut Response<'_>| Err(refused(unsupported_write())),
        )?,
    };
    Ok(router)
}
