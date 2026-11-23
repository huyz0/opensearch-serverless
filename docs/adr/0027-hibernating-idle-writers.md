# 0027: An idle writer hibernates: engine closed, head kept

**Status:** Proposed

## Context

A node holds every shard it ever wrote, engine in memory, until it stops: nothing closes an idle writer (R8.3). At 10M tenants of about ten indices each, a node that served a shard once keeps its heap for the life of the process, so memory is bounded only by how many shards a node ever touched. The shell closes idle writers two ways, and gives the head back after a long idle (300 s by default). Giving it back makes the next write pay a takeover: a head swap, the predecessor's lease and log reads, a new term. In this node that is 14 requests for a one-segment shard (emulator).

## Decision

- A pass over the held shards (run by the publish timer) hibernates a shard unused for `serverless.activation.hibernate_after_millis`: it publishes what no commit holds, then, under the slot lock and only if nothing reached the shard meanwhile and everything is published, closes the engine and records the term its head carried. The head is kept.
- A write to a hibernated shard reads the head (one GET). If it still names this process at that term, the shard is opened over its published commit at that term: no swap, no new term, no replay. Otherwise the ordinary open is made. A write that took the slot just before it slept finds it asleep and wakes the shard.
- Idleness is read from memory: a pass over shards none of which is idle makes no request. A wake is 5 requests, all reads, against 14 for a takeover.
- **A switch, default 0 (never).** A wake costs requests a shard that stays open does not, so by the switch rule it is off until measured to cost nothing it saves. Giving the head back after a longer idle is a later slice.

## Consequences

- Memory of an idle shard is freed without losing the head's ownership, so placement does not churn. A hibernated shard is not held here: a reader's freshness ask finds nothing in memory and reads the register, and a refresh answers ok because everything is published.
- The head still names a node holding nothing open; a node holding many such heads takes them all with it if it dies, which the later give-back bounds.
- A write that finds the shard asleep pays 5 requests the first time after a quiet period. At a threshold of seconds this is rare; at a threshold near zero a just-woken shard can be put back to sleep before its write lands and the write is refused (retry).
- Rules out waking at a head that moved: the term and process id are checked, so a take by another node while this one slept is seen.
