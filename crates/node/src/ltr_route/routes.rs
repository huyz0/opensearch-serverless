//! The route table of the store: each path under `/_ltr` and under
//! `/_plugins/_ltr`, with and without a store name.

use std::sync::Arc;

use oss_rest::{Method, Request, Response, RestError, RouteError, Router};

use super::{LtrService, Object};

/// The query parameters a store request accepts: the plugin's own, and the
/// timeouts every admin route takes.
const PARAMS: &[&str] = &[
    "prefix",
    "from",
    "size",
    "master_timeout",
    "cluster_manager_timeout",
    "timeout",
];

type Handler = fn(&LtrService, &mut Request<'_>, &mut Response<'_>) -> Result<(), RestError>;

/// `(methods, path under the root, handler)`; `{store}` paths come twice.
fn table() -> Vec<(&'static [Method], &'static str, Handler)> {
    use Method::{Delete, Get, Post, Put};
    vec![
        (&[Put, Post], "", LtrService::create_store),
        (&[Delete], "", LtrService::delete_store),
        (&[Get], "", LtrService::list_stores),
        (
            &[Put, Post],
            "/_featureset/{name}",
            LtrService::put_featureset,
        ),
        (&[Get], "/_featureset/{name}", |s, q, r| {
            s.get(Object::FeatureSet, q, r)
        }),
        (&[Delete], "/_featureset/{name}", |s, q, r| {
            s.delete(Object::FeatureSet, q, r)
        }),
        (&[Get], "/_featureset", |s, q, r| {
            s.page(Object::FeatureSet, q, r)
        }),
        (
            &[Post],
            "/_featureset/{name}/_addfeatures",
            LtrService::add_features,
        ),
        (&[Post], "/_featureset/{name}/_createmodel", |s, q, r| {
            s.put_model(true, q, r)
        }),
        (&[Put, Post], "/_model/{name}", |s, q, r| {
            s.put_model(false, q, r)
        }),
        (&[Get], "/_model/{name}", |s, q, r| {
            s.get(Object::Model, q, r)
        }),
        (&[Delete], "/_model/{name}", |s, q, r| {
            s.delete(Object::Model, q, r)
        }),
        (&[Get], "/_model", |s, q, r| s.page(Object::Model, q, r)),
    ]
}

/// The store routes.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: &Arc<LtrService>) -> Result<Router, RouteError> {
    let mut router = router;
    for root in ["/_ltr", "/_plugins/_ltr"] {
        for (methods, tail, handler) in table() {
            let paths = if tail.is_empty() {
                // The store routes: `/_ltr` and `/_ltr/{store}`.
                vec![root.to_owned(), format!("{root}/{{store}}")]
            } else {
                vec![format!("{root}{tail}"), format!("{root}/{{store}}{tail}")]
            };
            for path in paths {
                // The router keeps its patterns for the life of the process: a
                // handful of startup strings, leaked once.
                let path: &'static str = Box::leak(path.into_boxed_str());
                for &method in methods {
                    let service = Arc::clone(service);
                    router = router.route(
                        method,
                        path,
                        PARAMS,
                        move |q: &mut Request<'_>, r: &mut Response<'_>| handler(&service, q, r),
                    )?;
                }
            }
        }
    }
    Ok(router)
}
