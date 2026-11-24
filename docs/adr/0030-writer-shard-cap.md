# 0030: A writer node holds at most a set number of shards and lets an idle one go to open the next

**Status:** Accepted after independent review of the change, 2026-11-24

## Context

A writer takes every shard a write names and keeps it until the process ends or hibernation closes it ([ADR 0027](0027-hibernating-idle-writers.md), off by default). The busiest node grows without bound while nodes that joined later hold nothing. The shell bounds this with `ShardAdmission`: a cap (`maxShardsHeld`, default 1,000) and `makeRoom`, which lets go of a shard unused for a minute before a new one is refused.

## Decision

- `serverless.activation.max_shards`, default 0: no cap. A node at the cap asked to open a shard first makes room, and refuses with 503 when it cannot.
- `serverless.activation.evict_after_millis`, default 60,000, 0 never: a shard must be unused that long to be given up, so a node whose shards are all hot refuses rather than thrashing.
- Making room publishes the idlest shard's commit when writes are unpublished, closes it only if nothing reached it meanwhile, removes it, and gives its head back (a read and a conditional swap). A head that cannot be released leaves the shard recorded asleep at its term, as in ADR 0027, so its next write wakes it. One shard a call; no headroom fraction, no hand-off to a member with room (the shell's other two uses of the cap), not yet.
- **A switch, default off.** The soak held 10,000 shards on four cores at about 112 KB each, so a fixed default would refuse work a node can take. The cap is a deployment's figure; the default flips only once a fleet's own cap and its refusal rate are measured.

## Consequences

- Cap off: no change, no request. Cap on and room under it: no request. At the cap an eviction costs the idle shard's publish when it has unpublished writes, plus 2 requests for the head; reopening it costs a take. It scales with shards opened past the cap, never with elapsed time or idle shards.
- Evictions run under the role's opening lock, so they delay other opens while they run.
- Rules out evicting a shard used within `evict_after_millis`, and evicting without publishing first.
