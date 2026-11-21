//! `POST|GET /{index}/_refresh` over the shards this node writes: each
//! shard of the index is refreshed, and the answer counts them as
//! `RestRefreshAction` does.

use std::io::Write;
use std::sync::Arc;

use oss_rest::{JSON, Length, Method, Request, Response, RestError, RouteError, Router};

use crate::get_route::index_not_found;
use crate::writer::WriterRole;

/// Serves `_refresh` over a [`WriterRole`].
pub struct RefreshService {
    role: Arc<WriterRole>,
}

impl RefreshService {
    /// A service refreshing shards of `role`.
    pub fn new(role: Arc<WriterRole>) -> Self {
        Self { role }
    }

    fn handle(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let name = req.path_param("index").unwrap_or_default();
        let index = self
            .role
            .index_info(&name)
            .ok_or_else(|| index_not_found(&name))?;
        let mut successful: u32 = 0;
        for shard in 0..index.shards {
            let done = self.role.refresh_owned(&index, shard).is_ok();
            successful = successful.saturating_add(u32::from(done));
        }
        let mut out = resp.start(200, Some(JSON), Length::Chunked, &[])?;
        write!(
            out,
            "{{\"_shards\":{{\"total\":{},\"successful\":{successful},\"failed\":{}}}}}",
            index.shards,
            index.shards.saturating_sub(successful)
        )?;
        out.flush()?;
        out.finish()
    }
}

/// The routes of `_refresh`: by `POST` and `GET`.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: Arc<RefreshService>) -> Result<Router, RouteError> {
    let mut router = router;
    for method in [Method::Post, Method::Get] {
        let service = Arc::clone(&service);
        router = router.route(
            method,
            "/{index}/_refresh",
            &[],
            move |req: &mut Request<'_>, resp: &mut Response<'_>| service.handle(req, resp),
        )?;
    }
    Ok(router)
}
