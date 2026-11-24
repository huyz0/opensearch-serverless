# Hibernating a writer that has gone idle

## Serves

Parity plan R8.3 (shard activation and eviction, hibernation of idle writers), invariants I-1 (every store call metered; idle makes zero requests) and the bounded-memory rule. A node holds every shard it ever wrote, each with its engine in memory, for as long as it runs. Ported from `ShardHibernation` and `WriterHibernation` in the shell at `3920dc36`.

## Change

`oss-node` gains the hibernated state of a writer: the engine is closed and its memory freed while the shard's head stays this node's, so the next write wakes it at the same term instead of taking it over.

- **A pass over the held shards.** `WriterRole::hibernate_idle` closes each held shard whose last write or read has been `hibernate_after` ago (`serverless.activation.hibernate_after_millis`, node scope, default 0 = never, `OSS_HIBERNATE_AFTER_MS`). The publish timer calls it after its publish pass. A pass over shards none of which is idle makes no request: idleness is read from memory.
- **Only what is all published.** A shard with writes no commit holds is published first (its ordinary publish: the segment blob and the register swap, the log trimmed as after any publish), and sleeps only if no write reached it meanwhile; one that was written to again stays open and is tried at a later pass.
- **Waking.** A write to a hibernated shard reads the head (one GET). When it names this process at the term the shard slept at, the shard is opened over its published commit at that term and the write proceeds: no head swap, no new term, no replay of this node's log. When the head names anyone else or another term, the ordinary open is made (acquire, restore, replay). Exactly one write wakes it: the opener already runs one at a time.
- **A write that raced the hibernation** (it took the shard's slot just before it slept) finds the slot asleep, lets it go and wakes the shard, rather than failing.
- **Reads.** A refresh of a hibernated shard answers ok (nothing is unpublished). A get wakes the shard, as a write does, and answers from it, so a document that read before the shard slept reads after.
- A node that restarts has a new ephemeral id and no memory of what slept, so a head it left is taken the ordinary way.
- Not in this change: giving the head back after a longer idle (`serverless.activation.idle_after_millis`), warming the block cache before closing, and the writer mark of rollups.

## Acceptance criteria

- AC-1: with the setting 0 a pass hibernates nothing and makes no request, however long a shard has been idle.
- AC-2: a published shard unused for `hibernate_after` hibernates: it is no longer held, its engine's memory is freed, its head still names this node, and the pass made no store request.
- AC-3: a shard used (written, or read) within `hibernate_after` stays open.
- AC-4: a shard with writes no commit holds is published by the pass and then sleeps; the documents are all in the published commit a reader opens.
- AC-5: a write to a hibernated shard wakes it at the same term: every document written before and after is found, the head's term is unchanged, and the wake makes fewer requests than releasing the shard and taking it over would (the numbers are rows of the request budgets).
- AC-6: when the head names another node by the time a hibernated shard is asked for, the shard is opened the ordinary way (the write is refused with the holder, as for any shard that node holds).
- AC-7: writes interleaved with passes that put the shard to sleep all land, and every document is in the shard afterwards: a write that took the shard's slot just before it slept wakes it rather than being applied to the closed engine.
- AC-8: a refresh of a hibernated shard answers ok and makes no request; a get wakes the shard and finds the document.
- AC-9: an assembled node with the threshold set hibernates an idle, published shard on its publish timer; with it unset no shard is ever hibernated.
- AC-10: `OSS_HIBERNATE_AFTER_MS` sets the threshold, defaults to 0 (never), and a value that is not a number is refused.
- AC-11: A hibernated shard's record is forgotten once it has been asleep for a day (`KEEP_ASLEEP_MILLIS`), so the shards of deleted indexes do not stay in memory; forgetting asks nothing, and a write that does come takes the shard the ordinary way.

## Test plan

`crates/node/tests/writer_hibernation.rs` over the emulator and a real writer role (the rig of `proactive_takeover`), a fake clock for the idle time; AC-9 through `assemble` over the emulator with a real timer, AC-10 on `NodeConfig`.
