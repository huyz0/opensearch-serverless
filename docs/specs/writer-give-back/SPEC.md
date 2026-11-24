# A hibernated shard gives its head back

## Serves

[ADR 0029](../../adr/0029-giving-back-a-hibernated-head.md), the later slice [ADR 0027](../../adr/0027-hibernating-idle-writers.md) left: a head kept for a shard that holds nothing open is released after a longer sleep. Parity plan R8.3, invariant I-1 (idle makes no request until the shard is due).

## Change

- `ShardHeadStore::release(index, shard, node_id, ephemeral_id, term, uuid)` clears the owner of one head that still names this process at that term (and that uuid) with a swap conditional on the generation read: 2 requests. The term, uuid and writers stay. It answers whether it released.
- `WriterRole::give_back_idle(after, now)` claims the record of every shard asleep for `after`, and asks the opener to release each head; `give_back_due(now)` runs it at the role's threshold and does nothing at 0. The publish timer calls it after the hibernate pass. `HibernatePass.given_back` counts the heads released.
- The setting `serverless.activation.give_back_after_millis` (node scope, default 0 = never, `OSS_GIVE_BACK_AFTER_MS`), read at boot.

## Acceptance criteria

- AC-1: `release` clears the owner of a head this process owns at that term, keeps the term, uuid and writers, and costs one GET and one swap (request-budget row `writer.hibernation.give_back.requests`).
- AC-2: `release` leaves a head another node owns, one at another term or another process, or one of another incarnation, unchanged, and answers false.
- AC-3: a shard asleep for the threshold has its head released: the owner is cleared, the term is unchanged, and the shard's record is gone; a shard asleep less is left alone and a pass over none due makes no request.
- AC-4: with the threshold 0, nothing is given back however long a shard has slept.
- AC-5: a write to a shard whose head was given back opens it the ordinary way at the next term, and every document written before and after is found.
- AC-6: another node's write takes a given-back head at once, with no liveness revocation, and this node's later ask for the shard is refused with that holder.
- AC-7: a write that woke the shard first leaves nothing for the give-back: it releases nothing and the head still names this node.
- AC-8: an assembled node with both thresholds set gives back the head of an idle published shard on its timer; with the give-back threshold unset it never does.
- AC-9: `OSS_GIVE_BACK_AFTER_MS` sets the threshold, defaults to 0, and a value that is not a number is refused.

## Test plan

`crates/cluster/tests/leases/heads.rs` for AC-1 and AC-2; `crates/node/tests/writer_give_back.rs` over the emulator and a real writer role (the rig of `writer_hibernation`) for AC-3 to AC-7; AC-8 through `assemble` with a real timer; AC-9 on `NodeConfig`.
