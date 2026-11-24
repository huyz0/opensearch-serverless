# A writer node holds at most a set number of shards and makes room by letting an idle one go

## Serves

R13.2: the Java shell's `ShardAdmission` (`maxShardsHeld`, `makeRoom`) for writer activations. A node that takes every shard a write names holds them until the process ends, so a fleet's busiest node grows without bound and the nodes that joined later hold nothing. The cap bounds what one node holds; at the cap it gives up a shard nobody has used lately before it refuses.

## Change

- `serverless.activation.max_shards` (node scope, `OSS_MAX_SHARDS`), default 0: no cap. A node at the cap, asked to open a shard it does not hold, first makes room, and refuses with 503 `unavailable_shards_exception` ("at its shard cap") when it cannot.
- `serverless.activation.evict_after_millis` (node scope, `OSS_EVICT_AFTER_MS`), default 60,000, 0 never evicts: how long a held shard must be unused before a node at its cap lets it go. A cap with eviction at 0 only refuses.
- `WriterRole::make_room(now)`: picks the held shard unused longest, if it has been unused for `evict_after_millis`; publishes its commit when writes are unpublished; closes it only if nothing reached it meanwhile (as hibernation does); removes it; gives its head back. If the head cannot be given back, the shard is recorded as asleep so the next write wakes it at the same term. One shard a call.
- Counters: `WriterRole::shard_cap_stats()` answers the cap, shards held, activations refused at the cap, and shards evicted for room.
- A hibernated shard holds no engine and does not count towards the cap. Waking one counts, as an open does.

Not in this change: reader-side admission, handing shards to a member with room, steering writes to it, write holds, the headroom fraction (Java evicts below the cap to cut how often it searches), pinned indexes.

## Acceptance criteria

- AC-1: with no cap, every shard a write names is opened and none is refused or evicted,.
- AC-2: at the cap, a shard unused for `evict_after_millis` is evicted to open the next: the new shard is held, the old is not, and the count of held shards equals the cap.
- AC-3: at the cap with every held shard used within `evict_after_millis`, the open is refused with a message naming the cap, nothing is evicted, and the refusal is counted.
- AC-4: with `evict_after_millis` 0 a node at the cap refuses and evicts nothing.
- AC-5: the shard evicted loses no write: its next write reopens it with every earlier document, and its head names this node again.
- AC-6: the evicted shard's head is given back (it names no owner) when the release succeeds; when it does not, the shard is asleep and wakes at the same term.
- AC-7: the settings are registered with the defaults above, and `OSS_MAX_SHARDS` and `OSS_EVICT_AFTER_MS` set them and refuse a value that is not a number.

## Test plan

`crates/node/tests/writer_shard_cap.rs` over a writer role on the emulator with explicit clocks (AC-1 to AC-6); the node's settings registry test and `crates/node/tests/server/boot.rs` (AC-7).

## S3 request budget

Cap off: unchanged, zero. Cap on, a node under it: unchanged. At the cap an eviction costs the shard's publish (when it has unpublished writes) and one head release (a read and a conditional PUT); reopening it later costs a take (`writer.hibernation.wake.requests` is the cheaper path when the release failed). The cost scales with shards opened beyond the cap, never with elapsed time or with idle shards: an idle node at its cap makes no request.

Held by `evicting_a_published_shard_costs_one_put_and_a_shard_with_unpublished_writes_one_more` (an eviction of a published shard is one PUT, the release, after the head's read; two requests) and by the refusal test (a refusal writes nothing and reads at most the index descriptor).

## Switch

`serverless.activation.max_shards`, default 0 (off). A fixed default would refuse the 1,001st shard on a node that held ten thousand at 112 KB each in the soak, so the default stays off until a fleet's own cap is measured; the Java's 1,000 is a deployment's figure, not a universal one.

## Risks

- With hibernation and give-back off (their defaults), a shard evicted whose head could not be released stays recorded asleep, its head naming this node, until it is written again or the record is a day old; the record count is bounded by those failed releases.
- An eviction under steady demand above the cap thrashes: each open evicts what the next one needs. `evict_after_millis` is the guard, as in the Java: a shard used within it is never evicted, so a node whose shards are all hot refuses, and the refusal counter reads that as capacity.
- Opens are serialised by the role's opening lock, so an eviction's requests delay other opens while it runs.
