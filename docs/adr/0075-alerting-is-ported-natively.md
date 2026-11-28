# 0075: Alerting is ported natively; it is no longer proxied

**Status:** Proposed (needs an independent review before it is accepted)

## Context

[ADR 0046](0046-plugin-requests-are-proxied-to-java-nodes.md) hands the alerting prefixes to a Java node. The owner decided on 2026-11-28, as for learning to rank in [ADR 0071](0071-learning-to-rank-is-ported-natively.md), that the stock plugins are ported to Rust. Alerting differs from learning to rank in one way that matters: it does work with no request in flight. A monitor runs on a schedule, and the stock plugin does it with the Job Scheduler plugin's timer and a system index that is polled.

Forces: idle indices make zero requests; no lock, lease or leader election for correctness; requests scale with nodes and bytes, never with elapsed time per index; memory is reserved before it is held ([ADR 0004](0004-bounded-memory-and-streaming.md)).

## Decision

Alerting is native in `oss-meta` (the store), `oss-reader` (monitor evaluation) and `oss-node` (the routes and the runner), as [the alerting spec](../specs/alerting/SPEC.md) lays out.

- **State is blobs under `alerting/`**, written by compare-and-swap and never listed: monitors, destinations and channels, and alerts, each a marker register naming immutable content blobs (the stored-scripts shape).
- **No poll.** A monitor's next run is derived from its schedule and the clock (`next = first slot after the last claimed slot`); a node keeps the due-times of the monitors it has loaded in memory and sleeps until the earliest, so an idle monitor set costs no request.
- **A run is claimed by a conditional create** of `alerting/runs/<monitor>/<slot>`: one PUT per run, whichever node wins. No lock and no lease; a paused or partitioned node that loses the create skips the run. Which nodes load which monitors is the existing placement hash, so most runs have one candidate, and the create decides the rest.
- **A monitor is a search** the node runs through the ordinary reader path, so its cost is the search's cost and is charged to the tenant of the monitor's indices.
- Notifications go out over HTTP from the node to the webhook, Slack, Chime or custom URL, bounded in size and time, never from a timer that has nothing to send.

## Consequences

- Easier: alerting without a Java node; no hop; runs are visible in the request meter.
- Harder: the cost of an enabled monitor is its schedule times its search (a once-a-minute monitor is 1,440 searches a day by design); the spec sets a floor on the interval and a per-tenant count bound.
- Rules out: Java nodes reading the Rust store; painless trigger scripts beyond what the reader's painless supports.
- The proxy keeps ISM, anomaly detection and ML Commons; the alerting prefixes leave its list when the routes land.
