# 0034: The tail overlay is on by default

**Status:** Accepted by the owner's standing decision to ship it on (the shell does), after an independent review whose blocking findings are answered in Not measured; 2026-11-24

## Context

The tail overlay (a search served over the open commit and the acknowledged writes pushed above it) was built off by default until takeover was covered and its cost measured. [ADR 0032](0032-commit-push-in-a-booted-node.md) and its takeover suite cover the first. The shell ships it on.

## Decision

`serverless.search.tail_overlay.enabled` defaults to `true`; the per-index `index.serverless.search.tail_overlay.enabled` stays. The build rule's bars, measured on the three-node freshness meter with the overlay on against off, 100 to 300 samples a series, one run each (docs/specs/overlay-default-on/VERIFIED.md): the write's ack p99 is lower with it on (336 ms against 343 at refresh=false, 400 against 500 at refresh=true), a refreshed write is visible in 0.3 s against 1.4 to 2 s, searches answer in 2.6 to 4.7 ms p50 against 167 to 456 ms, and the requests a write fall (3.2 GETs against 20, 2.5 PUTs against 9.9, no range GETs against 111 to 321). The commit push's send to other nodes defaults to the opposite; only the pair of defaults was measured together.

## Not measured

Memory and timers across many shards (the overlay builder charges `open_indexes` without evicting; the tail keep and push queues are per-shard bounded and the flush timer is per node, but no run with thousands of shards has been made), how often a reader refuses a search as behind and for how long over a long run (the refusal's release has no test of its own), and the `serving` and `owner` visibilities with the overlay on. A deployment that cares sets `OSS_TAIL_OVERLAY=false` or the index setting.

## Consequences

- A node that wants the old behaviour sets `OSS_TAIL_OVERLAY=false` or the index setting.
- A reader that cannot hold the tail (no room, a refused record) serves its commit and refuses a search as behind its writer until it is let go (ADR 0032).
- Tests that pinned the old default set the variable they pinned.
