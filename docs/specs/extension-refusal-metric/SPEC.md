# Extension refusal metric

## Serves

Roadmap R15.3, [extension-analyzer](../extension-analyzer/SPEC.md) AC-18 and [ADR 0053](../../adr/0053-the-analyzer-extension-point.md) section 4: a mis-deployed fleet "shows as unserved shards, not wrong results" and a metric. Reads through [prometheus-metrics](../prometheus-metrics/SPEC.md).

## Change

A node counts each open of a pinned index that its module source refused, and `GET /_prometheus/metrics` serves the totals:

- `oss_extension_open_refused_total{reason="busy"}`: the extensions pool had no room to compile a module (a retry may succeed).
- `oss_extension_open_refused_total{reason="unavailable"}`: the pins do not read, a module is missing, does not match its hash, or cannot be an analyzer.

Both samples are always present (zero until counted). The count is one atomic add where the module source returns a refusal, for the writer's open and the reader's open alike; it is per process, which is per node. Nothing is counted for a cached refusal's reuse differently: each refused open counts once. Not here: a label per index or module (the series count would scale with them), and a metric for the proactive takeover's deferrals, which the pass's report already carries.

## Acceptance criteria

- AC-1: an open refused for want of pool room counts once under `busy`, and any other refusal once under `unavailable`; an open that succeeds, and an index with no pins, count nothing.
- AC-2: the metrics body has `oss_extension_open_refused_total` with a `reason` sample for `busy` and for `unavailable`, with HELP and TYPE lines, and the samples move by the refusals counted.
- AC-3: a scrape that serves the family makes no blob request.

## Test plan

- AC-1: `crates/node/tests/extension_modules/refusals.rs`, over the module source rig: refuse an open for a missing module and for a full pool, and assert the totals move by one each; a good open moves neither. Red first: no totals exist.
- AC-2: `crates/node/tests/metrics_route.rs`: the body has both samples at zero, and after `record` they read the counts.
- AC-3: the existing scrape test of the ledger's total before and after (prometheus-metrics AC-6) runs with the family present and is unchanged.

## S3 request budget

Unchanged: one atomic add on an error path, one atomic read per sample on a scrape. No timer, no background job.

## Switch

None: a read-only counter has no downside to switch off.

## Risks

A process holding several nodes (a test) shares the totals; tests assert deltas. A refusal counted per open means a shard retried every few seconds counts every try, which is what makes a wrongly placed shard visible.
