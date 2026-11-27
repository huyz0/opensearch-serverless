# Mapping growth: a document's new fields into the descriptor

## Serves

Parity plan R6.4 (roadmap stage 3, [roadmap](../../roadmap.md)), the fencing rule (a register swap, never a lock) and the request-cost rule (the one register write a data path makes is bounded). Builds on [mappings](../mappings/SPEC.md) and [document indexing](../indexing/SPEC.md), whose `parse` returns the grown mapping; the descriptor store is [descriptors](../descriptor-store/SPEC.md).

## Change

`oss-writer` gains `mapping_growth` (`crates/writer/src/mapping_growth/`), ported from `MappingGrowthPath` (huyz0/OpenSearch `feature/serverlessshell`):

- `MappingGrowth::grow(index, uuid, addition)` adds what a document needed to the index's mapping in its descriptor, durably, and answers the mapping the descriptor then holds. The descriptor is the mapping's home, so this is the same compare-and-swap `PUT /{index}/_mapping` makes, which moves `mapping_version` on by one.
- **Attempts**: five. Each reads the descriptor and its generation (one read), refuses an index that is gone or has another uuid, merges every addition into the descriptor's mapping, and swaps over the generation. A lost swap waits and tries again; attempt `n` after the first waits a random `1..=min(500, 20 << min(n, 6))` ms. A mapping that is already what the merge gives is answered without a swap (another node grew it).
- **Coalescing**: per index on this node. Callers queue their additions; the first to take the index's turn drains the queue and swaps once for all of them; a caller whose addition was already drained answers with the mapping that leader wrote. A batch the store failed goes back on the queue for the callers waiting on it; once the group has no caller left, what is queued is dropped (it belongs to failed requests). A batch the mapping refuses is answered one addition at a time, so the compatible ones still grow the mapping and only the incompatible one's caller is refused (an incompatible addition must not poison every later growth); a batch that finds the index gone or loses every swap is answered to all its callers alike. The group is dropped once no caller is in it.
- **Refusals**: a merge the mapping refuses is `Conflict`, carrying the Java's message, and is not retried; five lost swaps is `Concurrent` ("retry the write"); an index deleted or recreated is `Deleted`.
- The wait is injected, so tests run without sleeping.

Not changed: the reconciler refresh and descriptor hosting of the Java (`node.reconciler`, `descriptorRefresh`), which are the node's, and the shard adopts the answer; refresh scheduling from the indexing buffer is the next piece of this milestone.

## Acceptance criteria

- AC-1: growing an index's mapping writes the merged mapping to its descriptor, moves `mapping_version` by one, and answers the merged mapping; the other descriptor fields are unchanged.
- AC-2: an addition the mapping already has makes no swap and no version move.
- AC-3: a swap lost to another writer reads again and publishes over what it finds, so two writers of different fields both keep theirs; the waits follow `min(500, 20 << min(n, 6))` with a jitter inside `1..=` that bound.
- AC-4: five lost swaps fail with `Concurrent`, having waited four times.
- AC-5: a merge the mapping refuses is `Conflict` with the Java's message after one read, with no swap and no wait; a later growth is not affected.
- AC-6: an index that is absent, a tombstone, an alias, or has another uuid is `Deleted`, with no swap.
- AC-7: concurrent callers for one index coalesce: of N threads each adding its own field, every field is in the descriptor and the swaps made are fewer than N; a batch the store failed goes back to the callers waiting on it, who grow the mapping with it, and is dropped with the group when nobody waits (a failed request's addition is not kept); of two callers whose additions are batched, one incompatible, the other still grows the mapping.

## Test plan

`crates/writer/tests/it/mapping_growth.rs` over the in-process emulator with a request meter and a recording wait (AC-1 to AC-6); the same file with threads and a gate on the store for AC-7.

## S3 request budget

One growth with no contention: 1 register read and 1 swap, whatever the number of additions it carries. Each lost swap adds one read and one swap. A node's rate is bounded by the field limit, which is why the write is allowed here at all. Nothing scales with documents or time.

## Switch

None: the Java does this unconditionally on a write that names a field the mapping lacks.

## Risks

- Two ways of changing a mapping drifting: this is the one path `PUT /_mapping` also uses (the same merge, the same swap).
- A caller waiting forever on a poisoned group: a failed batch is requeued, and a panic in a leader leaves the turn lock to be recovered, not dropped.
