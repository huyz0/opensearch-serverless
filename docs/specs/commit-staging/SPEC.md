# Commit staging: a routine publish keeps its manifest, the register is a checkpoint

## Serves

Parity plan R11.2, first slice (log commits and checkpoints), under [ADR 0064](../../adr/0064-commits-are-staged-and-checkpointed.md). Ported from the shell's `CommitStaging` and its use in `SegmentPublisher` (huyz0/OpenSearch `feature/serverlessshell`). Logged commits and opening from them are later slices with their own specs.

## Change

- `ShardPublisher::use_staging(Some(checkpoint))` turns staging on for its next publishes (`None`, the default, off). `publish_routine` is `publish` unless staging is on, the publisher holds the register it last wrote in this term as this writer, and the oldest staged commit is younger than the checkpoint age: then it uploads the commit's fresh files and keeps the manifest, with no register swap. `publish` always writes the register and clears what was staged.
- `flush_due` swaps the newest staged manifest over the generation the publisher last wrote when the oldest staged commit is as old as the checkpoint age; `flush_staged` does it whatever the age. Neither uploads. A staged commit older than the pack deadline is dropped and `PublishError::PackExpired` is returned; its files are uploaded again by the next publish.
- A staged commit's manifest carries a `stage_seq` that rises with each commit this publisher stages or writes while staging is on (the shell's field; `-1` with staging off, as now).
- `Published.staged` says the manifest is staged, not in the register; its `generation` is the register's.
- Settings `serverless.wal.node_log.commits.staging` (node scope, default `false`) and `serverless.wal.node_log.commits.checkpoint_millis` (default 60,000; `check(checkpoint, commit_period)` bounds it as the shell's `CommitStaging.check`: at least 1,000 ms and two commit periods, at most half the pack deadline).
- The node's writer role: a routine publish that stages does not trim the log, advance the lazy policy's published mark or tell the tail overlay it may drop what the commit holds; the log position it covers is kept per shard and trimmed when a register write lands (a checkpoint or any full publish). A shard with a staged commit counts as unpublished, so hibernation, eviction and hand-off publish it in full first. `publish_due` also writes each shard's checkpoint when it is due, with no snapshot. Releasing a shard forgets its staged commit.

Not changed: the reader, the janitor, takeover, the write log's format, node packs, buckets (a member's register swap is its own, and is staged and flushed the same way).

## Acceptance criteria

- AC-1: With staging off, `publish_routine` makes the same requests and writes the same manifest bytes as `publish`.
- AC-2: With staging on, the first routine publish of a term writes the register (the fence).
- AC-3: With staging on, a routine publish inside the checkpoint age makes exactly one blob PUT and no register PUT, reports `staged`, and leaves the register's generation and bytes as they were.
- AC-4: A staged commit's files are not uploaded again: the next publish uploads only files it does not already hold.
- AC-5: A routine publish whose oldest staged commit has reached the checkpoint age swaps the register with its own manifest, naming every file of the commit, and every named blob exists.
- AC-6: `flush_due` writes the staged manifest with one register PUT and no upload when due, does nothing when not due or when nothing is staged.
- AC-7: `publish` after staged commits writes the register with all files and uploads none of the staged ones again.
- AC-8: A staged commit past the pack deadline is never written: `flush_due` drops it with `PackExpired`, and the next publish uploads its files again.
- AC-9: A register moved by a newer term makes the flush `Stale` and leaves nothing staged.
- AC-10: A staged commit's `stage_seq` rises with every commit while staging is on.
- AC-11: The checkpoint age check accepts its bounds and refuses a value just outside each.
- AC-12: The two settings are in the node's registry with these defaults.
- AC-13: On the node, a staged publish does not trim the log, mark the shard published or let the tail drop records, and the register write that follows trims it through the newest position; a shard with a staged commit is published in full before hibernation and before eviction.
- AC-14: Crash: a node staging commits is dropped with writes acknowledged after its last register write; its successor opens and every acknowledged write is searchable, none doubled.

## Test plan

AC-1 to AC-10 in `crates/writer/tests/it/publish_staging.rs` over the emulator, counting PUTs by the meter (the bug each catches: a staged commit that swaps anyway, a re-upload of staged files, a manifest that names a file not uploaded). The due boundary and the deadline are unit tests of the pure functions in `publish/staging.rs` (a `>=` against a `>` is what a mutation of them would flip). AC-11 and AC-12 as unit tests beside the settings. AC-13 and AC-14 in `crates/node/tests/roles/` with the writer-role rigs the takeover and publish suites use.

## S3 request budget

Staging on: a routine publish inside the checkpoint age drops from one blob PUT + one register PUT to one blob PUT (a pack PUT with packs on); the checkpoint is one PUT per shard a checkpoint age and reads nothing (the generation is remembered). No GET, LIST or HEAD is added anywhere. Staging off: unchanged.

## Switch

`serverless.wal.node_log.commits.staging`, default `false`. It flips on the build rule's measurements (write p99 within 5%, no extra requests, no search regression, the crash tests), taken in the cost soak (R11.5), not here. The existing tests pin the old behaviour; the new tests cover staging on.

## Risks

A staged commit lost to a crash costs a successor up to one checkpoint age more replay (measured in the soak). The log is kept that much longer. A reader that reads only the register is that far behind a writer that does not push. A bug that trims the log behind a staged commit would lose acknowledged writes: AC-13 and AC-14 hold it.
