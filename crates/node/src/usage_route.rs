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
use std::time::Duration;

use crate::assemble::{AssembleError, Assembled};
use crate::tenants::TenantLevels;
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
    tenants: Option<Arc<TenantLevels>>,
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
            tenants: None,
        }
    }

    /// This service also reporting the levels of `tenants`.
    #[must_use]
    pub fn with_tenants(mut self, tenants: Arc<TenantLevels>) -> Self {
        self.tenants = Some(tenants);
        self
    }

    fn handle(&self, req: &Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let top = match req.param("top") {
            None => None,
            Some(text) => match text.parse::<usize>() {
                Ok(n) if n >= 1 => Some(n),
                _ => {
                    return Err(RestError::illegal_argument(format!(
                        "[top] must be a whole number of at least 1, got [{text}]"
                    )));
                }
            },
        };
        // ALLOC: the body, one entry an index the ledger tracks.
        let mut out = Vec::new();
        self.render(&mut out, top)?;
        resp.send(200, JSON, &out)
    }

    fn render(&self, out: &mut Vec<u8>, top: Option<usize>) -> Result<(), RestError> {
        out.write_all(b"{\"node_id\":")?;
        write_json_str(&mut *out, &self.node_id)?;
        write!(
            out,
            ",\"started_millis\":{},\"now_millis\":{},\"metered\":true,\"indices\":{{",
            self.started_millis,
            (self.clock)()
        )?;
        let mut lines = self.ledger.lines();
        if let Some(n) = top {
            costliest(&mut lines, n);
        }
        for (i, (name, usage)) in lines.iter().enumerate() {
            if i > 0 {
                out.write_all(b",")?;
            }
            write_json_str(&mut *out, name)?;
            out.write_all(b":")?;
            line(out, usage)?;
        }
        out.write_all(b"}")?;
        if let Some(tenants) = &self.tenants {
            self.tenants_json(out, tenants, top)?;
        }
        out.write_all(b"}")?;
        Ok(())
    }

    fn tenants_json(
        &self,
        out: &mut Vec<u8>,
        tenants: &TenantLevels,
        top: Option<usize>,
    ) -> std::io::Result<()> {
        out.write_all(b",\"tenant_levels\":{\"mode\":\"")?;
        out.write_all(crate::tenants::word(tenants.mode()).as_bytes())?;
        out.write_all(b"\",\"tenants\":{")?;
        let mut report = tenants.report((self.clock)());
        if let Some(n) = top {
            report.sort_by(|a, b| {
                b.1.cost_per_minute
                    .total_cmp(&a.1.cost_per_minute)
                    .then_with(|| a.0.cmp(&b.0))
            });
            report.truncate(n);
        }
        for (i, (name, t)) in report.iter().enumerate() {
            if i > 0 {
                out.write_all(b",")?;
            }
            write_json_str(&mut *out, name)?;
            write!(
                out,
                ":{{\"level\":{},\"local_level\":{},\"set_level\":{},\"budget_per_minute\":{},\"cost_per_minute\":{:.3},\"docs_per_minute\":{:.3},\"paced\":{},\"refused\":{}}}",
                t.level,
                t.local_level,
                t.set_level,
                tenants.budget_of(name),
                t.cost_per_minute,
                t.docs_per_minute,
                t.paced,
                t.refused
            )?;
        }
        out.write_all(b"}}")
    }
}

/// Keeps the `n` costliest indices, highest first, then the lines that are
/// no index's (`_node`, `_other`), which are always listed.
fn costliest(lines: &mut Vec<(String, Arc<IndexUsage>)>, n: usize) {
    let split = lines.len().saturating_sub(2);
    let mut rest = lines.split_off(split);
    lines.sort_by(|a, b| {
        b.1.cost_milli_units()
            .cmp(&a.1.cost_milli_units())
            .then_with(|| a.0.cmp(&b.0))
    });
    lines.truncate(n);
    lines.append(&mut rest);
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
        &["top"],
        move |req: &mut Request<'_>, resp: &mut Response<'_>| service.handle(req, resp),
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
/// when store usage is off. With tenant levels on, steps them from the ledger
/// and reports them.
///
/// # Errors
///
/// [`AssembleError::Route`] when the route is registered twice.
pub fn attach(
    assembled: Assembled,
    ledger: Option<Arc<Ledger>>,
    config: &crate::boot::NodeConfig,
    clock: &Arc<dyn oss_cluster::LeaseClock>,
) -> Result<Assembled, AssembleError> {
    let Some(ledger) = ledger else {
        return Ok(assembled);
    };
    let reader = Arc::clone(clock);
    let now: Arc<dyn Fn() -> i64 + Send + Sync> = Arc::new(move || reader.now_millis());
    let mut service = UsageService::new(
        Arc::clone(&ledger),
        &config.node_id,
        clock.now_millis(),
        Arc::clone(&now),
    );
    let mode =
        oss_blob::guard::Mode::parse(config.tenant_levels).unwrap_or(oss_blob::guard::Mode::Off);
    if mode != oss_blob::guard::Mode::Off {
        let tiers = crate::tenants::TierBudgets {
            small: config.tenant_budget_small,
            standard: config.tenant_budget_standard,
            large: config.tenant_budget_per_minute,
        };
        let default = crate::tenants::Tier::parse(config.tenant_default_tier).unwrap_or(
            if mode == oss_blob::guard::Mode::Enforce {
                crate::tenants::Tier::Standard
            } else {
                crate::tenants::Tier::Large
            },
        );
        let levels = Arc::new(
            TenantLevels::new(ledger, mode, config.tenant_budget_per_minute)
                .with_tiers(tiers, default),
        );
        let noted = Arc::clone(&levels);
        assembled
            .role
            .set_tenancy_sink(Arc::new(move |index, t| noted.note_tenancy(index, t)));
        let stretched = Arc::clone(&levels);
        let at = Arc::clone(&now);
        assembled.role.set_publish_stretch(Arc::new(move |index| {
            stretched.publish_stretch(index, at())
        }));
        let paced = Arc::clone(&levels);
        let when = Arc::clone(&now);
        assembled.role.set_write_gate(Arc::new(move |index, docs| {
            paced.admit_write(index, docs, when())
        }));
        levels.spawn_ticker(Duration::from_millis(config.tenant_tick_millis), now);
        service = service.with_tenants(levels);
    }
    assembled.with_usage(service)
}
