//! `GET /_serverless/usage`: what this node's work has cost the object
//! store, by index.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/rest/UsageHandler.java`.
//! Counters only grow, so a reader takes the difference between two reads.
//! Requests are decimals (the ledger keeps thousandths); `cost_units` is in
//! PUTs, a GET counting 0.08. `_node` is what no index made; `_other` the
//! indices past the ledger's limit.

use std::io::Write;
use std::sync::Arc;

use crate::assemble::{AssembleError, Assembled};
use oss_blob::S3Op;
use oss_blob::usage::{IndexUsage, Ledger};
use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router, write_json_str};

/// The request kinds in the order they are written, with their names.
const KINDS: [(S3Op, &str); 7] = [
    (S3Op::Get, "get"),
    (S3Op::GetRange, "get_range"),
    (S3Op::Head, "head"),
    (S3Op::Put, "put"),
    (S3Op::Copy, "copy"),
    (S3Op::List, "list"),
    (S3Op::Delete, "delete"),
];

/// The usage of one node.
pub struct UsageService {
    ledger: Arc<Ledger>,
    node_id: String,
    started_millis: i64,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl UsageService {
    /// A service over `ledger` for `node_id`, started at `started_millis`.
    pub fn new(
        ledger: Arc<Ledger>,
        node_id: &str,
        started_millis: i64,
        clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        Self {
            ledger,
            node_id: node_id.to_owned(), // ALLOC: one name, at start.
            started_millis,
            clock,
        }
    }

    fn handle(&self, resp: &mut Response<'_>) -> Result<(), RestError> {
        // ALLOC: the body, one entry an index the ledger tracks.
        let mut out = Vec::new();
        self.render(&mut out)?;
        resp.send(200, JSON, &out)
    }

    fn render(&self, out: &mut Vec<u8>) -> Result<(), RestError> {
        out.write_all(b"{\"node_id\":")?;
        write_json_str(&mut *out, &self.node_id)?;
        write!(
            out,
            ",\"started_millis\":{},\"now_millis\":{},\"metered\":true,\"indices\":{{",
            self.started_millis,
            (self.clock)()
        )?;
        for (i, (name, usage)) in self.ledger.lines().iter().enumerate() {
            if i > 0 {
                out.write_all(b",")?;
            }
            write_json_str(&mut *out, name)?;
            out.write_all(b":")?;
            line(out, usage)?;
        }
        out.write_all(b"}}")?;
        Ok(())
    }
}

/// `1234` thousandths as `1.234`.
fn milli(out: &mut Vec<u8>, thousandths: u64) -> std::io::Result<()> {
    write!(
        out,
        "{}.{:03}",
        thousandths.checked_div(1_000).unwrap_or(0),
        thousandths.checked_rem(1_000).unwrap_or(0)
    )
}

fn line(out: &mut Vec<u8>, usage: &IndexUsage) -> std::io::Result<()> {
    out.write_all(b"{\"cost_units\":")?;
    milli(out, usage.cost_milli_units())?;
    out.write_all(b",\"requests\":{")?;
    for (i, (op, name)) in KINDS.iter().enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        write!(out, "\"{name}\":")?;
        milli(out, usage.requests(*op))?;
    }
    write!(
        out,
        "}},\"bytes\":{{\"read\":{},\"written\":{}}}}}",
        usage.bytes_read(),
        usage.bytes_written()
    )
}

/// The usage route.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: Arc<UsageService>) -> Result<Router, RouteError> {
    router.route(
        Method::Get,
        "/_serverless/usage",
        &[],
        move |_req: &mut Request<'_>, resp: &mut Response<'_>| service.handle(resp),
    )
}

impl Assembled {
    /// Registers `GET /_serverless/usage` over `service`.
    ///
    /// # Errors
    ///
    /// [`AssembleError::Route`] when the route is registered twice.
    pub fn with_usage(mut self, service: UsageService) -> Result<Self, AssembleError> {
        self.router = routes(self.router, Arc::new(service)).map_err(AssembleError::Route)?;
        Ok(self)
    }
}

/// Serves the usage of `ledger` when there is one; `assembled` unchanged
/// when store usage is off.
///
/// # Errors
///
/// [`AssembleError::Route`] when the route is registered twice.
pub fn attach(
    assembled: Assembled,
    ledger: Option<Arc<Ledger>>,
    node_id: &str,
    clock: &Arc<dyn oss_cluster::LeaseClock>,
) -> Result<Assembled, AssembleError> {
    let Some(ledger) = ledger else {
        return Ok(assembled);
    };
    let reader = Arc::clone(clock);
    assembled.with_usage(UsageService::new(
        ledger,
        node_id,
        clock.now_millis(),
        Arc::new(move || reader.now_millis()),
    ))
}
