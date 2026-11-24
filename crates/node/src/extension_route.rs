//! `PUT /_serverless/extensions/modules`: store a module under the hash of
//! its bytes, after checking it loads
//! ([ADR 0052](../../../docs/adr/0052-extension-modules-on-a-wasm-abi.md)).
//!
//! Not an OpenSearch endpoint. One upload at a time a node; the body is read
//! into a buffer reserved from the request-body pool while it is hashed.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use oss_blob::BlobStore;
use oss_ext::MAX_MODULE_BYTES;
use oss_ext_wasm::{Host, LoadError};
use oss_mem::MemoryBudget;
use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router};

use crate::extension_store::{self as store, PoolReserver, StoreError};
use crate::route_util::refuse;

/// Serves the module upload route.
pub struct ExtensionService {
    store: Arc<dyn BlobStore>,
    base: String,
    host: Host,
    budget: MemoryBudget,
    uploading: AtomicBool,
}

/// Frees the node's one upload place when the upload ends.
struct Place<'a>(&'a AtomicBool);

impl Drop for Place<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn failed(e: StoreError) -> RestError {
    match e {
        StoreError::Load(LoadError::Busy) => refuse(
            429,
            "extension_busy",
            "no capacity to compile the module; retry",
        ),
        // ALLOC: error path; the refusal's words.
        StoreError::Load(e) => refuse(400, "extension_rejected", e.to_string()),
        StoreError::Mismatch => refuse(
            500,
            "extension_corrupt",
            "a stored module does not match its hash",
        ),
        // ALLOC: error path; the store's words.
        StoreError::Blob(e) => refuse(503, "extension_store_failure", e.to_string()),
    }
}

impl ExtensionService {
    /// A service over `store` under `base`, whose compiles and calls are
    /// charged to `budget`'s `extensions` pool.
    pub fn new(store: Arc<dyn BlobStore>, base: &str, host: Host, budget: MemoryBudget) -> Self {
        Self {
            store,
            // ALLOC: one prefix a node.
            base: base.to_owned(),
            host,
            budget,
            uploading: AtomicBool::new(false),
        }
    }

    /// `PUT /_serverless/extensions/modules`.
    pub fn upload(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        if self
            .uploading
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(refuse(
                429,
                "extension_busy",
                "another module is being uploaded; retry",
            ));
        }
        let _place = Place(&self.uploading);
        let declared = req.body().declared_length();
        if declared.is_some_and(|n| n > u64::from(MAX_MODULE_BYTES)) {
            return Err(RestError::too_large());
        }
        // What the body declares, or the most a module may be; reserved and
        // allocated once, so the buffer never grows past what was reserved.
        let room = declared.map_or(MAX_MODULE_BYTES as usize, |n| {
            usize::try_from(n).unwrap_or(MAX_MODULE_BYTES as usize)
        });
        req.body().reserve(room)?;
        // ALLOC: the module, at most MAX_MODULE_BYTES, reserved above.
        let mut bytes: Vec<u8> = Vec::with_capacity(room);
        while let Some(chunk) = req.body().next_chunk()? {
            if bytes.len().saturating_add(chunk.len()) > room {
                return Err(RestError::too_large());
            }
            bytes.extend_from_slice(chunk);
        }
        let hash = store::hash_of(&bytes);
        let module = self
            .host
            .load(&bytes, Arc::new(PoolReserver(self.budget.clone())))
            .map_err(|e| failed(StoreError::Load(e)))?;
        let created =
            store::write_once(self.store.as_ref(), &self.base, &hash, &bytes).map_err(failed)?;
        let d = module.descriptor();
        let (a, b, c) = d.version();
        // ALLOC: one small body an upload.
        let body = format!(
            r#"{{"id":"{hash}","name":"{}","version":"{a}.{b}.{c}","created":{created}}}"#,
            d.name()
        );
        resp.send(if created { 201 } else { 200 }, JSON, body.as_bytes())
    }
}

/// Registers the upload route on `router`.
///
/// # Errors
///
/// [`RouteError`] for a route already taken.
pub fn routes(router: Router, service: Arc<ExtensionService>) -> Result<Router, RouteError> {
    router.route(
        Method::Put,
        "/_serverless/extensions/modules",
        &[],
        move |req: &mut Request<'_>, resp: &mut Response<'_>| service.upload(req, resp),
    )
}
