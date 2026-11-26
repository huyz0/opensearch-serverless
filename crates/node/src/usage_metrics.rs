//! `GET /_prometheus/metrics`: this node's counters in the Prometheus text
//! format (docs/specs/prometheus-metrics). The route reads what
//! [`UsageService`] already keeps: the ledger's totals, the memory budget's
//! pools and the search gate and queue reports. Nothing is counted for it on
//! a request path, and a scrape makes no blob request.

use std::io::Write;
use std::sync::Arc;

use oss_blob::S3Op;
use oss_mem::Pool;
use oss_rest::{Method, Request, Response, RestError, RouteError, Router};

use crate::usage_route::UsageService;

/// The media type of the text format's version 0.0.4.
const TEXT: &str = "text/plain; version=0.0.4; charset=utf-8";

/// The request kinds with their label values.
const OPS: [(S3Op, &str); 7] = [
    (S3Op::Get, "get"),
    (S3Op::GetRange, "get_range"),
    (S3Op::Head, "head"),
    (S3Op::Put, "put"),
    (S3Op::Copy, "copy"),
    (S3Op::List, "list"),
    (S3Op::Delete, "delete"),
];

/// The header of a family: its help and its type.
fn family(out: &mut Vec<u8>, name: &str, kind: &str, help: &str) -> std::io::Result<()> {
    writeln!(out, "# HELP {name} {help}")?;
    writeln!(out, "# TYPE {name} {kind}")
}

/// `1234` thousandths as `1.234`.
fn milli(thousandths: u64) -> String {
    // ALLOC: one short number a sample.
    format!(
        "{}.{:03}",
        thousandths.checked_div(1_000).unwrap_or(0),
        thousandths.checked_rem(1_000).unwrap_or(0)
    )
}

impl UsageService {
    /// The metrics of this node in the Prometheus text format.
    fn render_prometheus(&self, out: &mut Vec<u8>) -> std::io::Result<()> {
        family(
            out,
            "oss_s3_requests_total",
            "counter",
            "Object store requests this node made, by kind.",
        )?;
        for (op, name) in OPS {
            writeln!(
                out,
                "oss_s3_requests_total{{op=\"{name}\"}} {}",
                milli(self.ledger.total_requests(op))
            )?;
        }
        let (read, written) = self.ledger.total_bytes();
        family(
            out,
            "oss_s3_bytes_total",
            "counter",
            "Body bytes moved to and from the object store.",
        )?;
        writeln!(out, "oss_s3_bytes_total{{direction=\"read\"}} {read}")?;
        writeln!(out, "oss_s3_bytes_total{{direction=\"written\"}} {written}")?;
        let cost = self
            .ledger
            .lines()
            .iter()
            .fold(0u64, |sum, (_, u)| sum.saturating_add(u.cost_milli_units()));
        family(
            out,
            "oss_s3_cost_units_total",
            "counter",
            "Object store cost in PUT-priced units, a GET counting 0.08.",
        )?;
        writeln!(out, "oss_s3_cost_units_total {}", milli(cost))?;
        if let Some(budget) = &self.budget {
            family(
                out,
                "oss_memory_pool_limit_bytes",
                "gauge",
                "The most bytes a memory pool may hold.",
            )?;
            for pool in Pool::ALL {
                writeln!(
                    out,
                    "oss_memory_pool_limit_bytes{{pool=\"{}\"}} {}",
                    pool.name(),
                    budget.limit(pool)
                )?;
            }
            family(
                out,
                "oss_memory_pool_in_use_bytes",
                "gauge",
                "Bytes reserved from a memory pool now.",
            )?;
            for pool in Pool::ALL {
                writeln!(
                    out,
                    "oss_memory_pool_in_use_bytes{{pool=\"{}\"}} {}",
                    pool.name(),
                    budget.in_use(pool)
                )?;
            }
        }
        if let Some(slots) = &self.slots {
            let r = slots.report();
            for (name, kind, help, value) in [
                (
                    "slots",
                    "gauge",
                    "Searches the gate runs at once.",
                    r.slots as u64,
                ),
                ("in_use", "gauge", "Gate slots held now.", r.in_use as u64),
                (
                    "waiting",
                    "gauge",
                    "Searches waiting for a slot.",
                    r.waiting as u64,
                ),
                (
                    "admitted_total",
                    "counter",
                    "Searches admitted.",
                    r.admitted,
                ),
                (
                    "waited_total",
                    "counter",
                    "Searches that waited for a slot.",
                    r.waited,
                ),
                (
                    "past_wait_total",
                    "counter",
                    "Searches that waited past the limit.",
                    r.past_wait,
                ),
                (
                    "overflow_full_total",
                    "counter",
                    "Searches refused with no waiter room.",
                    r.overflow_full,
                ),
                (
                    "overflow_cap_total",
                    "counter",
                    "Searches refused at a tenant's cap.",
                    r.overflow_cap,
                ),
                (
                    "longest_wait_millis",
                    "gauge",
                    "The longest wait for a slot so far.",
                    r.longest_wait_millis,
                ),
            ] {
                family(out, &format!("oss_search_slots_{name}"), kind, help)?;
                writeln!(out, "oss_search_slots_{name} {value}")?;
            }
        }
        if let Some(queue) = &self.queue {
            let r = queue.report();
            for (name, kind, help, value) in [
                (
                    "running",
                    "gauge",
                    "Searches running now.",
                    r.running as u64,
                ),
                ("parked", "gauge", "Searches parked now.", r.parked as u64),
                (
                    "deferred_total",
                    "counter",
                    "Searches deferred.",
                    r.deferred,
                ),
                (
                    "resumed_total",
                    "counter",
                    "Parked searches resumed.",
                    r.resumed,
                ),
                ("refused_total", "counter", "Searches refused.", r.refused),
                (
                    "expired_total",
                    "counter",
                    "Parked searches that expired.",
                    r.expired,
                ),
                (
                    "dropped_total",
                    "counter",
                    "Parked searches dropped.",
                    r.dropped,
                ),
                (
                    "longest_wait_millis",
                    "gauge",
                    "The longest park so far.",
                    r.longest_wait_millis,
                ),
            ] {
                family(out, &format!("oss_search_queue_{name}"), kind, help)?;
                writeln!(out, "oss_search_queue_{name} {value}")?;
            }
        }
        Ok(())
    }

    fn handle_metrics(&self, resp: &mut Response<'_>) -> Result<(), RestError> {
        // ALLOC: the body, a few lines a family.
        let mut out = Vec::new();
        self.render_prometheus(&mut out)?;
        resp.send(200, TEXT, &out)
    }
}

/// The metrics route.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: Arc<UsageService>) -> Result<Router, RouteError> {
    router.route(
        Method::Get,
        "/_prometheus/metrics",
        &[],
        move |_: &mut Request<'_>, resp: &mut Response<'_>| service.handle_metrics(resp),
    )
}
