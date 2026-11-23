# A node takes over a shard that has a history

## Serves

R8.1 on the node: [shard opener](../node-shard-opener/SPEC.md) refused a shard with a published commit or a head that names earlier writers. With a `Takeover` configured, `HeadOpener` opens such a shard: it restores the published commit ([writer restore](../writer-restore/SPEC.md)), replays what the head's writers logged and did not publish, publishes under the new head's term, and only then holds the shard ([writer takeover](../writer-takeover/SPEC.md)).

## Change

- `oss-node` (feature `writer`) gains `Takeover` (the predecessors' logs, the head-writers settle, the replay's memory bound) and `HeadOpener::with_takeover`.
- `WriterRole::open_taken` opens the shard's engine over the restored files and, when the head names writers, runs `adopt` through the role's publisher before the shard is held. A failure holds nothing, answers the write 503 and the next write tries again.
- What the take published is recorded as the shard's newest commit and the lazy publish policy's published position, so readers are told of it and the timer does not publish it again.
- `Takeover::from_leases` builds the predecessors from the node's leases: for each node a head names it reads the node's lease register for the epochs of its log, fences them as the successor and checks each fence against the lease again; a node with no lease register has no log, and the head keeps naming it. `boot` passes it to the opener with a 16 MiB replay bound.
- Not changed: the opener without a `Takeover` refuses as before.

## Acceptance criteria

- AC-1: a node that wins the head of a shard whose previous writer published a commit and logged more restores the commit, replays the log and carries on: the next write's sequence number follows the last one logged, and the register holds the new term.
- AC-2: a shard whose predecessor published everything is restored and publishes no commit of its own.
- AC-3: a shard whose predecessor's log cannot be read is not held; every write to it answers 503, and the next write asks for the predecessor's log again.
- AC-5: `Takeover::from_leases` makes a successor replay the log its predecessor's lease names, as AC-1 does, and answers no log for a node with no lease register.
- AC-4: without a `Takeover`, a shard with a history is refused as takeover not supported, as before.

## Test plan

`crates/node/tests/roles/writer_takeover.rs` over the emulator with two nodes sharing a store: the successor judges leases an hour on, so the first node is dead to it. AC-4 stays in `writer_opener.rs`.

## S3 request budget

A take adds the predecessor's lease read (one register read a predecessor a batch, and one more after each fence), the restore (one register read, one GET a blob of the commit), the replay's log reads, one blob PUT and one register swap for the publish when anything was replayed, and the head's settle. LIST 0; nothing scales with documents or time.

## Switch

None: a shard with a history cannot serve correctly without it, and it runs only where `Takeover` is configured.

## Risks

- The whole shard is restored into memory, as the writer already holds the files it writes.
- A takeover with a very large unpublished log replays under `max_buffer_bytes` and publishes once at the end.
