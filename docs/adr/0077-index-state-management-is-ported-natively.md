# 0077: Index State Management is ported natively; it is no longer proxied

**Status:** Accepted (independent review, three rounds, 2026-11-28; the last found two cost and contention gaps, fixed in the spec)

## Context

[ADR 0046](0046-plugin-requests-are-proxied-to-java-nodes.md) hands the ISM prefixes to a Java node. The owner decided on 2026-11-28, as for learning to rank in [ADR 0071](0071-learning-to-rank-is-ported-natively.md) and alerting in [ADR 0075](0075-alerting-is-ported-natively.md), that the stock plugins are ported to Rust. ISM, like alerting, does work with no request in flight: the stock plugin runs every managed index's job on a fixed interval (five minutes) through the Job Scheduler plugin and a polled system index. That is a request per index per interval, which the rules forbid: requests scale with nodes and bytes, never with elapsed time per index.

lucene-rust holds no ISM code (its only related content is Lucene merge policies); the actions ISM takes (rollover, alias moves, blocks, snapshot, delete) are routes and protocols this repository already has.

## Decision

ISM is native in `oss-meta` (policies and managed-index records), `oss-node` (the routes and the runner) and the existing rollover, alias, block and snapshot services it calls, as [the ISM spec](../specs/ism/SPEC.md) lays out.

- **State is blobs under `ism/`**, written by compare-and-swap, never listed: policies and managed-index records, each a marker register naming immutable content blobs.
- **Evaluation is derived, not polled.** The next evaluation of a managed index is computed from its policy (a creation-age or rollover-age condition, a cron, a retry backoff); a node sleeps until the earliest. `min_doc_count` and size conditions would need a request per index over time, so they are refused at policy put, not polled; the owner may decide otherwise.
- **Discovery without LIST** by 256 registry markers and the managed markers of the tenants a node places, the shape alerting uses, with its add and remove protocol restated in the spec and its own copy built here (a later change merges the two).
- **A step is claimed by compare-and-swap of a small per-index run register** and its outcome written against the claim's etag: no lock, lease or leader. Actions are idempotent, so a step abandoned by a dead node is re-claimed after `step_timeout`, and the dead node's late write fails.
- Every action is bounded below the step timeout and idempotent on re-run, stated per action in the spec. Actions the serverless store cannot honour (force merge, index priority, close, open, shrink, allocation, size and count conditions) are refused at policy put, never at run.

## Consequences

- Easier: lifecycle policies without a Java node; a policy's cost is visible in the request meter.
- Harder: plugin-visible shapes are reproduced from documentation until a stock recording exists; the per-tenant managed-index bound (1,000) is a limit the owner may want changed after measurement, and a policy that rolls over on a document count is refused until a count derived from the write path exists.
- Rules out: Java nodes reading the Rust store; ISM history indices; rollup and transform jobs.
- The proxy keeps anomaly detection and ML Commons (alerting and ML Commons are decided in their own ADRs); the ISM prefixes leave its list when the default turns on, not before.
