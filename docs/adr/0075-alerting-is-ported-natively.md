# 0075: Alerting is ported natively; it is no longer proxied

**Status:** Proposed (needs an independent review before it is accepted)

## Context

[ADR 0046](0046-plugin-requests-are-proxied-to-java-nodes.md) hands the alerting prefixes to a Java node. The owner decided on 2026-11-28, as for learning to rank in [ADR 0071](0071-learning-to-rank-is-ported-natively.md), that the stock plugins are ported to Rust. Alerting differs from learning to rank in one way that matters: it does work with no request in flight. A monitor runs on a schedule, and the stock plugin does it with the Job Scheduler plugin's timer and a system index that is polled.

Forces: idle indices make zero requests; no lock, lease or leader election for correctness; requests scale with nodes and bytes, never with elapsed time per index; memory is reserved before it is held ([ADR 0004](0004-bounded-memory-and-streaming.md)).

## Decision

Alerting is native in `oss-meta` (the store), `oss-reader` (monitor evaluation) and `oss-node` (the routes and the runner), as [the alerting spec](../specs/alerting/SPEC.md) lays out.

- **State is blobs under `alerting/`**, written by compare-and-swap and never listed: monitors, destinations and channels, and alerts, each a marker register naming immutable content blobs (the stored-scripts shape).
- **No poll per monitor or index.** A monitor's next run is derived from its schedule and the clock; a node sleeps until the earliest due slot. Discovery is without LIST: 256 registry markers and the monitor markers of the tenants a node places (a tenant sits on its top 2 nodes by hash) are read once per `refresh_ttl` (300 s), a standing cost of 256 x nodes + 2 x tenants-with-monitors reads per TTL, never with indices or searches; about 1.05 million tenants is the design's ceiling. Missed slots are not backfilled.
- **A run is claimed by a compare-and-swap** of one marker per monitor, `alerting/runs/<monitor>`, from a lower slot to this one: one PUT per run, nothing to reclaim. No lock and no lease. Only the top 2 nodes by the placement hash try. The run's window is the slot's boundaries, not the clock, and a node runs a slot only after its end plus 5 s of skew allowance and 5 s of visibility lag, so no claim is early. Semantics are **at most once per slot**: a node that dies after its claim loses that slot, by design.
- **A monitor is a search** the node runs through the ordinary reader path, so its cost is the search's cost and is charged to the tenant of the monitor's indices.
- Notifications go out over HTTP from the node to the webhook, Slack, Chime or custom URL, bounded in size and time, never from a timer that has nothing to send.

## Consequences

- Easier: alerting without a Java node; no hop; runs are visible in the request meter.
- Harder: the cost of an enabled monitor is its schedule times its search (a once-a-minute monitor is 1,440 searches a day by design); the spec sets a 60 s interval floor, a per-tenant count bound and a fleet-wide cap; the worst case per tenant (1,000 monitors at the floor is 1.44 million searches a day) is stated there.
- Rules out: Java nodes reading the Rust store; painless trigger scripts beyond what the reader's painless supports.
- The proxy keeps ISM, anomaly detection and ML Commons; the alerting prefixes leave its list when the default turns on, not before (the spec's Switch section), so the existing proxy tests and the new native tests both run in CI.
