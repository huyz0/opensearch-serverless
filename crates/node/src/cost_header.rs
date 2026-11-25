//! The cost header of a request that asked for it
//! ([ADR 0061](../../../docs/adr/0061-a-request-reports-its-own-store-requests.md)):
//! the store requests made on this node for the request, read from the
//! scope the ledger's tap adds to.

use std::fmt::Write as _;
use std::sync::Arc;

use oss_blob::S3Op;
use oss_blob::cost::{self, RequestCost, ScopeGuard};
use oss_rest::cost::{CostScope, CostSession};

/// Begins a scope on the worker for each request that asks.
pub(crate) struct NodeCostScope {
    node_id: String,
}

impl NodeCostScope {
    pub(crate) fn new(node_id: &str) -> Self {
        Self {
            // ALLOC: one name, at assembly.
            node_id: node_id.to_owned(),
        }
    }
}

impl CostScope for NodeCostScope {
    fn begin(&self) -> Arc<dyn CostSession> {
        let counters = RequestCost::new();
        let guard = cost::enter(Some(Arc::clone(&counters)));
        // ALLOC: one session a request that asked, freed with its response.
        Arc::new(Session {
            counters,
            node_id: self.node_id.clone(),
            _scope: guard,
        })
    }
}

struct Session {
    counters: Arc<RequestCost>,
    node_id: String,
    /// Leaves the scope when the response is dropped, on this thread.
    _scope: ScopeGuard,
}

impl CostSession for Session {
    fn header_value(&self) -> String {
        let c = &self.counters;
        // ALLOC: one short value a request that asked.
        let mut out = String::with_capacity(160);
        for (name, op) in [
            ("get", S3Op::Get),
            ("get_range", S3Op::GetRange),
            ("head", S3Op::Head),
            ("put", S3Op::Put),
            ("copy", S3Op::Copy),
            ("list", S3Op::List),
            ("delete", S3Op::Delete),
        ] {
            let _ = write!(out, "{name}={};", c.requests(op));
        }
        let units = c.cost_milli_units();
        let _ = write!(
            out,
            "bytes_read={};bytes_written={};units={}.{:03};node={};scope=node",
            c.bytes_read(),
            c.bytes_written(),
            units / 1_000,
            units % 1_000,
            self.node_id,
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // docs/specs/request-cost-header AC-3
    #[test]
    fn the_value_names_every_kind_the_bytes_the_units_and_the_node() {
        let scope = NodeCostScope::new("n1");
        let session = scope.begin();
        let value = session.header_value();
        assert_eq!(
            value,
            "get=0;get_range=0;head=0;put=0;copy=0;list=0;delete=0;bytes_read=0;bytes_written=0;units=0.000;node=n1;scope=node"
        );
    }
}
