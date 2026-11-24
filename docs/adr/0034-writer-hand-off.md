# 0034: A writer node nearly at its cap, or far above the fleet's mean, hands idle shards to a member with room

**Status:** Accepted after independent review of the change, 2026-11-24

## Context

[ADR 0030](0030-writer-shard-cap.md) bounds what one writer holds. A node that joins, or restarts empty, still takes only what reaches it while full nodes stay full until their shards go idle. The shell's `handOffToMemberWithRoom` fixes that: a node nearly at its cap, when another live writer has room, lets go of idle shards so the next write for one is placed there. The shell measured that shedding on a timer with no member to take the work made every node give shards up and take them back, so it acts only when someone has room.

## Decision

- The node's lease reports `held` and `cap` in the renewal PUT it already makes; peers decide from those leases, with the decision in `oss_cluster::placement::FleetView`.
- A pass after each fleet refresh lets go of idle shards by the cap's eviction (publish if unpublished, close, give the head back): nearly full with a member with room, down to 80% of the cap; far above the mean with a member below it, down to 1.25 times the mean; at most a tenth of the cap a pass; only shards unused for `evict_after_millis`.
- **A switch, default off** (`serverless.activation.hand_off.enabled`, `OSS_HAND_OFF`). It does nothing without a cap, and until writes for a handed-off shard are steered to the member with room (a later piece) the node that let it go can take it back, so it flips on only after a multi-node run shows no write p99 or request-count movement.

## Consequences

- Lease renewals are unchanged in count and nearly in size; a pass that should not hand off makes no request.
- A shard handed off costs what a cap eviction costs, bounded by the per-pass limit.
- Rules out handing off with no member that reports room, and handing off a shard used within the eviction grace.
