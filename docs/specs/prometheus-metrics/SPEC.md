# Prometheus metrics

## Serves

Roadmap R15.5 (no Prometheus metrics); the cost guardrails of the plan ("per-request blob cost", request counters). After [store-usage-wiring](../store-usage-wiring/SPEC.md), whose ledger it reads. OpenSearch's prometheus-exporter plugin serves `GET /_prometheus/metrics`; this node serves the same path with its own families.

## Change

`GET /_prometheus/metrics` answers the Prometheus text format (`text/plain; version=0.0.4; charset=utf-8`) for one node, from counters the node already keeps; nothing new is counted on a request path.

- `oss_s3_requests_total{op}` counter, one sample for each of `get`, `get_range`, `head`, `put`, `copy`, `list`, `delete`: the ledger's totals over every index and `_node` and `_other` (the ledger keeps thousandths of a request, written as decimals).
- `oss_s3_bytes_total{direction="read"|"written"}` counter.
- `oss_s3_cost_units_total` counter: PUT-priced units, a GET counting 0.08, as the usage route reports.
- `oss_memory_pool_limit_bytes{pool}` and `oss_memory_pool_in_use_bytes{pool}` gauges for each pool of the memory budget.
- `oss_search_slots_*` and `oss_search_queue_*`, when the search gate and the queue are on: the report's fields as gauges (`in_use`, `waiting`, `parked`, `running`) and counters (`admitted`, `waited`, `past_wait`, `overflow_full`, `overflow_cap`, `deferred`, `resumed`, `refused`, `expired`, `dropped`).

Every family has `# HELP` and `# TYPE` lines, and a label value is escaped as the format asks. The route needs the usage ledger (`serverless.store_usage`'s metering, on by default as the usage route is); a node without one does not register it. It is admin-only like `/_serverless/usage`. A scrape holds no lock a request path takes and reads atomics only; its cost is one pass over the ledger's lines, bounded by the ledger limit.

Not here: per-index labels (the usage route has them; a label per index would make the series count scale with indices), histograms, and node-stats families other than the above.

## Acceptance criteria

- AC-1: the body has the content type `text/plain; version=0.0.4; charset=utf-8` and, for every family, a `# HELP` and a `# TYPE` line before its samples, each sample line `name{labels} value` with a finite decimal value.
- AC-2: after a known sequence of metered requests, `oss_s3_requests_total` has one sample for each op with the ledger's total, `oss_s3_bytes_total` the bytes moved, and `oss_s3_cost_units_total` the usage route's sum.
- AC-3: each memory pool has a limit and an in-use sample, and a reservation held raises `in_use` for its pool only.
- AC-4: the search gate and queue families appear only when they are on.
- AC-5: a node with no ledger does not register the route; the route is admin-only under the security rules.
- AC-6: a scrape makes no blob request.

## Test plan

- AC-1 to AC-3: `crates/node/tests/metrics_route.rs`, over the same ledger rig as `usage_route.rs`; AC-1's grammar check is a small line parser in the test.
- AC-4: the same file with and without the gate.
- AC-5: the authz table test (every registered route has a row) and a boot test.
- AC-6: the ledger's total before and after a scrape.

## S3 request budget

Unchanged: a scrape reads memory only. No timer, no background job.

## Switch

None: a read-only route over existing counters has no downside to switch off.

## Risks

The ledger's totals are in thousandths because of shared requests; a counter that is a decimal is valid in the format, and a scraper's `rate()` is unaffected. A new family name is a contract once dashboards use it: names are `oss_` prefixed and units in the name.
