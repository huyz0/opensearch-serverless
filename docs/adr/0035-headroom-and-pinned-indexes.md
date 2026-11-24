# 0035: A writer node can clear room in bursts and can pin indexes it never lets go of

**Status:** Accepted after independent review of the change, 2026-11-24

## Context

[ADR 0030](0030-writer-shard-cap.md) lets a node at its cap let one idle shard go per open. The shell also clears a few percent below the cap in one pass (`evictHeadroomFraction`, so a burst of opens pays one victim search) and lets an index be pinned so its shards are never let go of. This adds both as node settings.

## Decision

- `serverless.activation.evict_headroom_percent`, default 0: making room continues past the first shard to the cap less that share (at least one shard), while idle shards remain.
- `serverless.activation.pinned_indexes`, default empty: those indexes' shards are skipped by eviction, hand-off and hibernation. Read at start, per node, as the shell's `pin` is.
- Both off by default: headroom lets go of shards nobody needed room for yet, a request cost if demand stops, and no run has measured the saving.

## Consequences

- No new request path; each shard let go costs what ADR 0030 says.
- A pinned set can fill a node, which then refuses new opens.
- Changing pins takes a restart, and a durable per-index pin (a register and a REST surface) stays out.
