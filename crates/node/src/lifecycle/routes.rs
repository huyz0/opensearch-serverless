//! `GET /_serverless/ready` and `POST /_serverless/drain`.
//!
//! Ported from the shell's `ServerlessReadyHandler` and
//! `ServerlessDrainHandler`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router};

use super::Readiness;
use crate::lifecycle_bus::Bus;

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
    events: Bus,
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
        move |_: &mut Request<'_>, resp: &mut Response<'_>| {
            drain(&readiness, settle, &events, resp)
        },
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
    events: &Bus,
    resp: &mut Response<'_>,
) -> Result<(), RestError> {
    if !begin(readiness, events) {
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

/// Milliseconds since the epoch, zero if the clock is before it.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Starts the drain and, the first time, tells the plugins the node is
/// stopping; whether this call started it.
fn begin(readiness: &Readiness, events: &Bus) -> bool {
    let first = readiness.start_draining();
    if first {
        events.publish("node.stopping", "", "", now_ms());
    }
    first
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use oss_ext::lifecycle::Event;

    use super::*;
    use crate::lifecycle_bus::Events;

    #[derive(Default)]
    struct Rec(Mutex<Vec<String>>);

    impl Events for Rec {
        fn publish(&self, event: &Event) {
            self.0.lock().unwrap().push(event.kind.clone());
        }
    }

    #[test]
    fn the_first_drain_tells_the_plugins_and_a_second_does_not() {
        let rec = Arc::new(Rec::default());
        let bus = Bus::default();
        assert!(bus.set(rec.clone()));
        let readiness = Readiness::new();
        assert!(begin(&readiness, &bus));
        assert!(!begin(&readiness, &bus));
        assert_eq!(*rec.0.lock().unwrap(), ["node.stopping"]);
    }
}
