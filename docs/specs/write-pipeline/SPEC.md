# Writes to one shard share a commit window

## Serves

The write path of [node-bulk-route](../node-bulk-route/SPEC.md) and [node-doc-write-route](../node-doc-write-route/SPEC.md); decided in [ADR 0050](../../adr/0050-writes-to-one-shard-share-a-commit-window.md), which holds the design, its rules and its crash plan.

## Finding

A write group held its shard's slot lock across the wait for the node log, so two requests to one shard never shared a window: four clients writing one index took 1,015 ms a bulk, 16 took 3.8 s.

## Change

- `NodeLogCommitter::stage` joins records to the windows; `Staged::wait` waits for them and `Staged::finish` drives them to the end without unwinding. `commit` is the two.
- `OSS_WRITE_PIPELINE_DEPTH` (1 to 64) is how many groups of one shard may wait together. At 1 the slot lock is held across the wait as before. Above 1 a group applies and stages under the slot lock, waits without it, and finishes in the order it staged: the acknowledgement effects run under the slot lock, in order, only while no earlier group has failed. A failed group fails the shard's pipeline; the slot is released by the last group out.
- A publish, hibernation or eviction waits for the groups in flight and holds new ones off until its snapshot is taken (publish) or the shard is asleep and released (hibernate, evict). Readers of the shard's highest acknowledged sequence number read the number of the oldest group not yet acknowledged.
- A bulk with a declared length takes a share of the bulk budget twice its length (at least 64 KiB, at most a slice), so a node's small bulks no longer hold 8 MiB each and wait together.

## Acceptance criteria

- AC-1: records staged one after another share a window and one PUT, in staging order; staging none stages nothing; finishing a window writes it without a wait, and a failed one is reported to the wait, not as a panic.
- AC-2: at depth 1, four groups of one shard take four PUT latencies, as before.
- AC-3: at depth 8, four groups of one shard in flight together all land, each with its own sequence number, in far less than four PUT latencies.
- AC-4: a failed window fails every group in flight behind it; none is acknowledged; the slot is released by the last one out; a group arriving after the failure is refused, not hung; the shard takes writes again.
- AC-5: a retire while groups are in flight loses none of the acknowledged writes.
- AC-6: a panic while a group's acknowledgement runs leaves no thread hung and the slot released.
- AC-7: 16 clients writing one index at once, a depth of 16, finish within 1.2 s (serialised they need 4 s).
- AC-8: 64 clients with small bulks to indexes of their own finish within 1.2 s (eight at a time, the old budget share, they need 2 s).
- AC-9: a refresh while groups are in flight waits for their window, then publishes and returns.
- AC-10: a refresh after a failed window comes back, publishes nothing unlogged, and the shard takes writes again.
- AC-11: at depth 2, six groups of one shard take at least three windows in turn.
- AC-12: a node that takes over from a pipelined one finds every acknowledged write and numbers past the highest.

## Test plan

`crates/writer/tests/node_log_staged.rs` (AC-1), `crates/node/tests/writer_pipeline.rs` (AC-2 to AC-6, AC-9 to AC-11), `crates/node/tests/server/boot_write_concurrency.rs` (AC-7, AC-8, a booted node over a file store).

## S3 request budget

Unchanged in kind: a window is still one PUT. More groups ride one window, so a node makes fewer PUTs for the same writes.

## Switch

`OSS_WRITE_PIPELINE_DEPTH`, node scope, default 1 (today's behaviour). It moves to a deeper default when the ADR's remaining tests exist (forced wake order, the quiesce of publish, hibernate and evict, a crash and reopen, and the memory bound) and the measurements hold; the node suites already pass at depth 8 and p99 at one client is within 1% of depth 1.
