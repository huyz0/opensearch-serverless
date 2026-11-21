//! Points in time over REST: freeze an index, release views, list them.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/rest/PointInTimeHandler.java`
//! (its routes, refusals and answer shapes), over `oss-meta`'s
//! [`PointInTimeStore::capture`].

mod params;

use std::io::Write;
use std::sync::Arc;

use oss_format::looks_like_an_id;
use oss_meta::{CaptureError, DescriptorStore, MetaError, PitClock, PointInTimeStore};
use oss_rest::{
    Exception, JSON, Length, Method, Request, Response, RestError, RouteError, Router,
    write_json_str,
};

use crate::time_value::{DEFAULT_KEEP_ALIVE_MILLIS, MAX_KEEP_ALIVE_MILLIS, time_millis};
use params::{MAX_IDS, ids_of};

/// The query parameters a create takes; the unsupported ones are refused.
const CREATE_PARAMS: &[&str] = &[
    "keep_alive",
    "allow_partial_pit_creation",
    "routing",
    "preference",
    "ignore_unavailable",
    "expand_wildcards",
    "allow_no_indices",
];

fn refuse(status: u16, kind: &'static str, reason: impl Into<String>) -> RestError {
    RestError::exception(status, Exception::new(kind, reason))
}

fn unsupported(reason: impl Into<String>) -> RestError {
    refuse(501, "unsupported_point_in_time", reason)
}

fn failed(e: &MetaError) -> RestError {
    // ALLOC: error path; the failure's words.
    refuse(500, "point_in_time_failure", e.to_string())
}

/// Serves the point-in-time routes over the stores.
pub struct PitService {
    pits: PointInTimeStore,
    descriptors: DescriptorStore,
    clock: PitClock,
    buffer_bytes: usize,
}

impl PitService {
    /// A service over `pits` and `descriptors`, telling the time by `clock`
    /// (the one `pits` uses). Each request holds one buffer of
    /// `buffer_bytes`, charged to the request-body pool; a view that does not
    /// fit it is refused.
    pub fn new(
        pits: PointInTimeStore,
        descriptors: DescriptorStore,
        clock: PitClock,
        buffer_bytes: usize,
    ) -> Self {
        Self {
            pits,
            descriptors,
            clock,
            buffer_bytes,
        }
    }

    fn buffer(&self, req: &mut Request<'_>) -> Result<Vec<u8>, RestError> {
        req.body().reserve(self.buffer_bytes)?;
        // ALLOC: one buffer a request, buffer_bytes, reserved above.
        Ok(vec![0u8; self.buffer_bytes])
    }

    /// `POST /{index}/_search/point_in_time`.
    pub fn create(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let index = req
            .path_param("index")
            .ok_or_else(|| RestError::illegal_argument("an index is required"))?;
        let keep_alive = match req.param("keep_alive") {
            Some(v) => time_millis("keep_alive", &v)?,
            None => DEFAULT_KEEP_ALIVE_MILLIS,
        };
        if req.param("routing").is_some_and(|v| !v.is_empty()) {
            return Err(unsupported(
                "routing a PIT to a subset of shards is not supported",
            ));
        }
        if req.param("preference").is_some_and(|v| !v.is_empty()) {
            return Err(unsupported(
                "PIT preferences are not supported: capture covers every shard of one concrete index",
            ));
        }
        if keep_alive > MAX_KEEP_ALIVE_MILLIS {
            return Err(refuse(
                400,
                "keep_alive_too_long",
                "keep_alive may be at most 1h; a view pins a commit's files for as long as it lives",
            ));
        }
        if index.contains([',', '*', '?']) {
            // ALLOC: error path; the name.
            return Err(unsupported(format!(
                "a point in time here is taken over one index, and [{index}] names several; take one per index"
            )));
        }
        let mut buf = self.buffer(req)?;
        let now = (self.clock)();
        let view = self
            .pits
            .capture(&self.descriptors, &index, keep_alive, &mut buf)
            .map_err(|e| capture_refusal(&index, e))?;
        let shards = view.shards.len();
        let mut out = resp.start(200, Some(JSON), Length::Chunked, &[])?;
        out.write_all(b"{\"pit_id\":")?;
        write_json_str(&mut out, &view.id)?;
        write!(
            out,
            ",\"_shards\":{{\"total\":{shards},\"successful\":{shards},\"skipped\":0,\"failed\":0}},\"creation_time\":{now},\"index\":"
        )?;
        write_json_str(&mut out, &view.index)?;
        write!(out, ",\"keep_alive_millis\":{keep_alive}}}")?;
        out.flush()?;
        out.finish()
    }

    /// `DELETE /_search/point_in_time` with a body, and `/{id}`.
    pub fn delete(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let ids = match req.path_param("id") {
            Some(id) => vec![id.into_owned()],
            None => ids_of(req.body())?,
        };
        self.release(req, resp, ids)
    }

    /// `DELETE /_search/point_in_time/_all`.
    pub fn delete_all(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        self.release(req, resp, vec!["_all".to_owned()])
    }

    fn release(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
        ids: Vec<String>,
    ) -> Result<(), RestError> {
        let mut buf = self.buffer(req)?;
        let all = ids.iter().any(|id| id == "_all");
        let targets = if all {
            self.finished(&mut buf)?
                .into_iter()
                .map(|view| view.id)
                .collect()
        } else {
            ids
        };
        let mut released: Vec<(String, bool)> = Vec::with_capacity(targets.len().min(MAX_IDS));
        for id in targets {
            // One entry an id, as the Java's map has: a repeat is not
            // released twice.
            if released.iter().any(|(seen, _)| *seen == id) {
                continue;
            }
            // An id that is no id was never a view, so it is not found, not
            // a failure of the store.
            let existed = looks_like_an_id(&id)
                && self.pits.release(&id, &mut buf).map_err(|e| failed(&e))?;
            released.push((id, existed));
        }
        if !all && released.iter().all(|(_, existed)| !existed) {
            let named: Vec<&str> = released.iter().map(|(id, _)| id.as_str()).collect();
            // ALLOC: error path; the ids named.
            return Err(refuse(
                404,
                "pit_not_found",
                format!("no such point in time: {}", named.join(", ")),
            ));
        }
        let mut out = resp.start(200, Some(JSON), Length::Chunked, &[])?;
        out.write_all(b"{\"pits\":[")?;
        for (i, (id, existed)) in released.iter().enumerate() {
            if i > 0 {
                out.write_all(b",")?;
            }
            write!(out, "{{\"successful\":{existed},\"pit_id\":")?;
            write_json_str(&mut out, id)?;
            out.write_all(b"}")?;
        }
        out.write_all(b"]}")?;
        out.flush()?;
        out.finish()
    }

    /// `GET /_search/point_in_time/_all`.
    pub fn views(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let mut buf = self.buffer(req)?;
        let views = self.finished(&mut buf)?;
        let now = (self.clock)();
        let mut out = resp.start(200, Some(JSON), Length::Chunked, &[])?;
        out.write_all(b"{\"pits\":[")?;
        for (i, view) in views.iter().enumerate() {
            if i > 0 {
                out.write_all(b",")?;
            }
            out.write_all(b"{\"pit_id\":")?;
            write_json_str(&mut out, &view.id)?;
            write!(
                out,
                ",\"keep_alive\":{},\"index\":",
                view.expires_at_millis.saturating_sub(now)
            )?;
            write_json_str(&mut out, &view.index)?;
            out.write_all(b"}")?;
        }
        out.write_all(b"]}")?;
        out.flush()?;
        out.finish()
    }

    /// The live views that are finished: a capture still reading its
    /// manifests is not a view a caller took, and releasing it would drop the
    /// pin on the commits it is reading.
    fn finished(&self, buf: &mut [u8]) -> Result<Vec<oss_format::PointInTime>, RestError> {
        let mut live = self.pits.live(buf).map_err(|e| failed(&e))?;
        live.retain(|view| !view.capturing);
        Ok(live)
    }
}

/// The Java's answer for each way a capture is refused.
fn capture_refusal(index: &str, e: CaptureError) -> RestError {
    // ALLOC: error path; the answer's words.
    match e {
        CaptureError::NoSuchIndex => {
            refuse(404, "index_not_found", format!("no such index: {index}"))
        }
        CaptureError::IsAlias => unsupported(format!(
            "[{index}] is an alias, and a point in time here is taken over one index; take one over each of its indices"
        )),
        CaptureError::Unsupported(why) => unsupported(why),
        CaptureError::NothingPublished(shard) => refuse(
            409,
            "nothing_published",
            format!(
                "shard {shard} of {index} has published nothing yet, so there is no commit to freeze"
            ),
        ),
        CaptureError::Replaced => refuse(409, "point_in_time_conflict", e.to_string()),
        CaptureError::Meta(e) => failed(&e),
    }
}

/// The routes of points in time, in the Java's spellings.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: Arc<PitService>) -> Result<Router, RouteError> {
    let mut router = router;
    for path in ["/{index}/_search/point_in_time", "/{index}/_pit"] {
        let s = Arc::clone(&service);
        router = router.route(
            Method::Post,
            path,
            CREATE_PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| s.create(req, resp),
        )?;
    }
    let s = Arc::clone(&service);
    router = router.route(
        Method::Delete,
        "/_search/point_in_time/_all",
        &[],
        move |req: &mut Request<'_>, resp: &mut Response<'_>| s.delete_all(req, resp),
    )?;
    for path in [
        "/_search/point_in_time",
        "/_search/point_in_time/{id}",
        "/_pit/{id}",
    ] {
        let s = Arc::clone(&service);
        router = router.route(
            Method::Delete,
            path,
            &[],
            move |req: &mut Request<'_>, resp: &mut Response<'_>| s.delete(req, resp),
        )?;
    }
    let s = Arc::clone(&service);
    router.route(
        Method::Get,
        "/_search/point_in_time/_all",
        &[],
        move |req: &mut Request<'_>, resp: &mut Response<'_>| s.views(req, resp),
    )
}
