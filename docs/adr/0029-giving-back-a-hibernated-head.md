# 0029: A hibernated shard gives its head back after a longer sleep

**Status:** Proposed

## Context

[ADR 0027](0027-hibernating-idle-writers.md) frees the memory of an idle writer and keeps its head, so a wake is 5 reads. It left one cost: the head still names a node that holds nothing open, so a node holding many such heads takes them all with it when it dies, and each is then taken the ordinary way by a write or by the proactive pass after the lease has lapsed and been revoked. The shell gives the head back after a long idle (300 s by default) and so bounds that.

## Decision

- A shard asleep for `serverless.activation.give_back_after_millis` has its head released: one GET of the head and one conditional swap that clears the owner, leaving the term, the uuid and the writers as they are. The swap is conditional on the generation just read, and the read must still show this process as owner at the term the shard slept at and the same index uuid; anything else is left alone.
- A released head is an unowned head, which `ShardHeadStore::acquire` already takes at the next term without a liveness check. Any node's next write, this one's included, takes it the ordinary way. A node that dies after releasing leaves nothing to revoke.
- The shard's record in memory is claimed before the release, by the same removal a wake makes, so exactly one of a wake and a give-back wins. A write arriving between the claim and the swap opens the shard the ordinary way: either its take moves the generation and the release finds the head changed and does nothing, or the release lands first and the take finds an unowned head. Both end with one owner.
- Everything a released shard holds is already in a published commit (a shard sleeps only then), so a successor has no log of this node to read. The head does not name this node among the writers: a take that did would, in this same process, fence the log epoch it is writing to; a later take from this node names it as it does for any owner.
- **A switch, default 0 (never).** A write after a give-back costs the take that a wake does not (14 requests against 5 for a one-segment shard), so by the switch rule it is off until measured to cost nothing it saves. It acts only with hibernation on.

## Consequences

- A node's death leaves the heads of its long-asleep shards unowned, not owned by a dead node, so no later take pays the revocation.
- A shard that wakes after a give-back pays the take: a new term. Placement may move it, which is the point.
- The cost of a give-back is 2 requests, once, for a shard that is not written again for the sleep's length; a pass over shards none of which is due makes none.
- Rules out releasing a head the record does not prove is this process's, and releasing without the swap's condition.
- The record forgotten after a day ([ADR 0027](0027-hibernating-idle-writers.md)) is unchanged: with a give-back threshold below a day the head is released first.
