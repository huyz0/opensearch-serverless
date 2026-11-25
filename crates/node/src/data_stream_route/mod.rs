//! Data stream routes: make, show and remove one stream by name.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/indexadmin/DataStreamHandler.java`.
//! A stream is an alias record with a generation over backing indices named
//! `.ds-<name>-<six-digit generation>`. Every name is literal: a pattern would
//! need a listing, and a request's cost never depends on how many streams
//! exist.

use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use oss_coordinator::indices::rollover::backing_index_name;
use oss_format::AliasRecord;
use oss_meta::{
    AliasClock, AliasMutations, DescriptorStore, MetaError, PitClock, Target,
    validate_index_or_alias,
};
use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router};

use crate::index_delete::DeleteService;
use crate::index_route::IndexService;
use crate::route_util::{refuse, refused};

mod render;

/// Query parameters a request accepts and has nothing to do for.
const PARAMS: &[&str] = &["timeout", "master_timeout", "cluster_manager_timeout"];

const ACK: &[u8] = br#"{"acknowledged":true}"#;

/// Serves the data stream routes over the descriptors.
pub struct DataStreamService {
    index: Arc<IndexService>,
    deletes: Arc<DeleteService>,
    descriptors: DescriptorStore,
    clock: PitClock,
    buffer_bytes: usize,
    lease_ttl_millis: i64,
}

impl DataStreamService {
    /// A service that creates backing indices through `index` and removes
    /// them through `deletes`, over `descriptors`. A request holds register
    /// buffers of `buffer_bytes`, charged to the request-body pool, and an
    /// alias reservation lasts `lease_ttl_millis`.
    pub fn new(
        index: Arc<IndexService>,
        deletes: Arc<DeleteService>,
        descriptors: DescriptorStore,
        clock: PitClock,
        buffer_bytes: usize,
        lease_ttl_millis: i64,
    ) -> Self {
        Self {
            index,
            deletes,
            descriptors,
            clock,
            buffer_bytes,
            lease_ttl_millis,
        }
    }

    fn now(&self) -> i64 {
        i64::try_from((self.clock)()).unwrap_or(i64::MAX)
    }

    /// `PUT /_data_stream/{name}`.
    pub fn put(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let name = stream_name(req)?;
        let backing = backing_index_name(&name, 1);
        // What the whole request holds, taken before the backing index is
        // made so that undoing it cannot be refused: the alias steps' three
        // buffers and the removal's. The name read reuses the removal's
        // headroom, and the create reserves its own two.
        req.body().reserve(
            self.buffer_bytes
                .saturating_mul(3)
                .saturating_add(self.deletes.remove_bytes()),
        )?;
        if self.is_taken(&name)? {
            return Err(already_exists(&name));
        }
        let (uuid, timestamp) = self.index.create_backing(req, &backing, &name)?;
        let mut record = AliasRecord::new(&name, &[&backing]);
        record.data_stream = true;
        record.generation = 1;
        record.timestamp_field = Some(Cow::Borrowed(&timestamp));
        let made = self.mutate(|m| m.create_alias(&record).map(|_| ()));
        if let Err(e) = made {
            // The stream lost its name: the backing index made for it must
            // not stay, or every later attempt would find its name taken.
            self.discard(&backing, &uuid);
            return Err(match e {
                MetaError::AlreadyExists => already_exists(&name),
                MetaError::Superseded | MetaError::Contended => contended(&name),
                other => refuse(
                    500,
                    "data_stream_create_failure",
                    // ALLOC: error path; one message.
                    other.to_string(),
                ),
            });
        }
        resp.send(200, JSON, ACK)
    }

    /// `GET /_data_stream/{name}`.
    pub fn get(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let name = stream_name(req)?;
        req.body().reserve(self.buffer_bytes.saturating_mul(3))?;
        // ALLOC: three buffers a read, buffer_bytes each, reserved above.
        let mut buf = vec![0u8; self.buffer_bytes];
        let record = match self
            .descriptors
            .resolve(&name, &mut buf)
            .map_err(read_failed)?
            .target
        {
            Target::Alias(record) if record.data_stream => record.into_owned(),
            _ => return Err(missing(&name)),
        };
        // ALLOC: one entry a backing index, bounded by the record.
        let mut backing = Vec::with_capacity(record.indices.len());
        for index in &record.indices {
            let uuid = self
                .descriptors
                .get(index, &mut buf)
                .map_err(read_failed)?
                .map(|d| d.uuid.into_owned());
            backing.push((index.to_string(), uuid));
        }
        let template = match self.index.templates() {
            Some(sets) => {
                // ALLOC: two scratch buffers a read, buffer_bytes each, reserved above.
                let mut marker = vec![0u8; self.buffer_bytes];
                let mut doc = vec![0u8; self.buffer_bytes];
                sets.inherited(&name, &mut marker, &mut doc)
                    .map_err(refused)?
                    .and_then(|i| i.from)
            }
            None => None,
        };
        let body = render::stream(&record, &backing, template.as_deref());
        resp.send(200, JSON, body.as_bytes())
    }

    /// `DELETE /_data_stream/{name}`.
    pub fn delete(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let name = stream_name(req)?;
        req.body().reserve(
            self.buffer_bytes
                .saturating_mul(4)
                .saturating_add(self.deletes.remove_bytes()),
        )?;
        // ALLOC: one buffer a delete, buffer_bytes, reserved above.
        let mut buf = vec![0u8; self.buffer_bytes];
        let Some(at) = self
            .mutate(|m| m.resolve_for_mutation(&name))
            .map_err(read_failed)?
        else {
            return Err(missing(&name));
        };
        if !at.alias.data_stream {
            return Err(missing(&name));
        }
        // The uuids of the incarnations this record names: a name reused
        // after the stream is gone is not this delete's to remove.
        // ALLOC: one entry a backing index, bounded by the record.
        let mut backing = Vec::with_capacity(at.alias.indices.len());
        for index in &at.alias.indices {
            if let Some(d) = self.descriptors.get(index, &mut buf).map_err(read_failed)? {
                backing.push((index.to_string(), d.uuid.into_owned()));
            }
        }
        let deleted = self
            .mutate(|m| m.delete_alias(&at.alias, at.generation))
            .map_err(read_failed)?;
        if !deleted {
            return Err(changed(&name));
        }
        // ALLOC: the names of the indices not removed, only on a failure.
        let mut failed: Vec<String> = Vec::new();
        for (index, uuid) in &backing {
            // `false` is an index another request already deleted or
            // replaced under the name: not this delete's to remove.
            if let Err(e) = self.deletes.remove(index, uuid) {
                failed.push(format!("[{index}]: {e}"));
            }
        }
        if !failed.is_empty() {
            return Err(refuse(
                500,
                "data_stream_delete_failure",
                // ALLOC: error path; one message.
                format!(
                    "data stream [{name}] was deleted; these backing indices were not, delete them by name: {}",
                    failed.join(", ")
                ),
            ));
        }
        resp.send(200, JSON, ACK)
    }

    /// Whether `name` already holds an index or an alias: one read.
    fn is_taken(&self, name: &str) -> Result<bool, RestError> {
        // ALLOC: one buffer a request, buffer_bytes, reserved above.
        let mut buf = vec![0u8; self.buffer_bytes];
        Ok(!matches!(
            self.descriptors
                .resolve(name, &mut buf)
                .map_err(read_failed)?
                .target,
            Target::Absent
        ))
    }

    /// Best effort: deletes the backing index a failed create made.
    fn discard(&self, backing: &str, uuid: &str) {
        // Reserved by the put before the index was made.
        let _ = self.deletes.remove(backing, uuid);
    }

    /// Runs `run` over alias mutations, whose three buffers the caller
    /// reserved.
    fn mutate<R>(
        &self,
        run: impl FnOnce(&mut AliasMutations<'_>) -> Result<R, MetaError>,
    ) -> Result<R, MetaError> {
        let now = || self.now();
        let pause = |d: Duration| std::thread::sleep(d);
        let mut mutations = AliasMutations::new(
            &self.descriptors,
            AliasClock {
                now_millis: &now,
                pause: &pause,
            },
            self.lease_ttl_millis,
            self.buffer_bytes,
        );
        run(&mut mutations)
    }
}

/// The literal stream name of the request, checked as the shell checks it.
fn stream_name(req: &Request<'_>) -> Result<String, RestError> {
    let name = req
        .path_param("name")
        .ok_or_else(|| RestError::illegal_argument("a data stream name is required"))?;
    if name.contains(['*', ',']) || name == "_all" {
        return Err(refuse(
            501,
            "unsupported_data_stream_name",
            // ALLOC: error path; one message.
            format!(
                "[{name}] is a pattern or a list: data streams are addressed by literal name, a pattern needs a listing"
            ),
        ));
    }
    if name.starts_with('_') || name.starts_with('.') {
        let underscore = name.starts_with('_');
        return Err(refuse(
            if underscore { 501 } else { 400 },
            if underscore {
                "not_implemented"
            } else {
                "illegal_argument_exception"
            },
            // ALLOC: error path; one message.
            format!("'{name}' is not a data stream name"),
        ));
    }
    validate_index_or_alias(&name)
        // ALLOC: error path; one message.
        .map_err(|e| refuse(400, "illegal_argument_exception", e.to_string()))?;
    Ok(name.into_owned())
}

fn already_exists(name: &str) -> RestError {
    // ALLOC: error path; one message.
    refuse(
        400,
        "resource_already_exists_exception",
        format!("[{name}] already exists"),
    )
}

/// The refusal for a record that changed between its read and its delete.
fn changed(name: &str) -> RestError {
    // ALLOC: error path; one message.
    refuse(
        409,
        "version_conflict_engine_exception",
        format!("data stream [{name}] was changed concurrently; nothing was deleted, retry"),
    )
}

fn contended(name: &str) -> RestError {
    // ALLOC: error path; one message.
    refuse(
        409,
        "alias_contended_exception",
        format!("data stream [{name}] changed while it was being created"),
    )
}

fn missing(name: &str) -> RestError {
    // ALLOC: error path; one message.
    refuse(
        404,
        "resource_not_found_exception",
        format!("data stream [{name}] does not exist"),
    )
}

fn read_failed(e: MetaError) -> RestError {
    // ALLOC: error path; one message.
    refuse(500, "data_stream_lookup_failure", e.to_string())
}

/// The data stream routes.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: Arc<DataStreamService>) -> Result<Router, RouteError> {
    let (put, get, del) = (
        Arc::clone(&service),
        Arc::clone(&service),
        Arc::clone(&service),
    );
    router
        .route(
            Method::Put,
            "/_data_stream/{name}",
            PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| put.put(req, resp),
        )?
        .route(
            Method::Get,
            "/_data_stream/{name}",
            &[],
            move |req: &mut Request<'_>, resp: &mut Response<'_>| get.get(req, resp),
        )?
        .route(
            Method::Delete,
            "/_data_stream/{name}",
            PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| del.delete(req, resp),
        )?
        .route(
            Method::Get,
            "/_data_stream",
            &[],
            move |_req: &mut Request<'_>, _resp: &mut Response<'_>| {
                Err(refuse(
                    501,
                    "unsupported_data_stream_name",
                    "listing every data stream needs a listing: name the data stream",
                ))
            },
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_changed_since_it_was_read_is_a_409_that_deleted_nothing() {
        let e = changed("logs-1");
        assert_eq!(e.status(), 409);
        let x = e.as_exception().expect("an exception");
        assert_eq!(x.kind(), "version_conflict_engine_exception");
        assert!(x.reason().contains("nothing was deleted"));
    }
}
