//! `GET /_serverless/ready` and `POST /_serverless/drain`.
//!
//! Ported from the shell's `ServerlessReadyHandler` and
//! `ServerlessDrainHandler`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router};

use super::Readiness;

/// Registers the two routes over `readiness`; a drain answers after `settle`,
/// which the node's configuration bounds to a minute.
///
/// # Errors
///
/// [`RouteError`] when a route is registered twice.
pub fn routes(
    router: Router,
    readiness: Arc<Readiness>,
    settle: Duration,
) -> Result<Router, RouteError> {
    let probe = Arc::clone(&readiness);
    let router = router.route(
        Method::Get,
        "/_serverless/ready",
        &[],
        move |_: &mut Request<'_>, resp: &mut Response<'_>| ready(&probe, resp),
    )?;
    router.route(
        Method::Post,
        "/_serverless/drain",
        &[],
        move |_: &mut Request<'_>, resp: &mut Response<'_>| drain(&readiness, settle, resp),
    )
}

fn ready(readiness: &Readiness, resp: &mut Response<'_>) -> Result<(), RestError> {
    if readiness.ready() {
        resp.send(200, JSON, br#"{"ready":true,"state":"ready"}"#)
    } else {
        let body = format!(r#"{{"ready":false,"state":"{}"}}"#, readiness.state());
        resp.send(503, JSON, body.as_bytes())
    }
}

fn drain(
    readiness: &Readiness,
    settle: Duration,
    resp: &mut Response<'_>,
) -> Result<(), RestError> {
    if !readiness.start_draining() {
        return resp.send(200, JSON, br#"{"drained":false,"already":true}"#);
    }
    let started = Instant::now();
    // The role leaves the lease at its next renewal; coordinators see it
    // within a refresh after that.
    std::thread::sleep(settle);
    let body = format!(
        r#"{{"drained":true,"took_millis":{}}}"#,
        started.elapsed().as_millis()
    );
    resp.send(200, JSON, body.as_bytes())
}
