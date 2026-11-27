# Logged commits: a staged commit rides the node log

## Serves

Parity plan R11.2, second slice, under [ADR 0064](../../adr/0064-commits-are-staged-and-checkpointed.md) point 6, after [commit-staging](../commit-staging/SPEC.md). Ported from the shell's `LoggedCommits` (huyz0/OpenSearch `feature/serverlessshell`). The writing half only: a successor reading the logged commit is the next slice, so until then a logged commit is never read and changes no behaviour but the log's contents.

## Change

- `oss_writer::log_commit` holds the key (`<uuid>#commit`, the shard's number and name), the record (`<oldest pack millis> <epoch> <ordinal>\n<manifest>`, epoch and ordinal `-1 -1` when the commit covers no log position) and `LoggedCommits`.
- `LoggedCommits::staged` enqueues the record under the commit key in the log's open window (`NodeLogCommitter::enqueue`: no write of its own when the node is logging). `tick` writes what nothing carried after two windows (`flush_queued`, one write for every shard waiting).
- Only the newest landed record is kept: when one lands, the one before it is released to the log's trim (`on_published`); a record older than the register's commit is released at once; `checkpointed` releases the held record the register now covers; `forget` drops a shard's.
- The node logs each staged commit when `serverless.wal.node_log.commits.logged` is on (default `false`, `OSS_COMMIT_LOGGED`; refused at start without staging) and calls `checkpointed` at every register write and `forget` when a shard is released.
- The commit key is not a shard: admission strips the suffix to find the real shard's head, and `NodeLogTakeover::named_within` leaves commit keys out, so neither proactive takeover nor the janitor sees a shard that has no head. A log holding only commit records is collectable.
- `ShardPublisher::staged_age` says how old the oldest staged commit's pack (or the first staged commit) is.

## Acceptance criteria

- AC-1: A staged commit is logged under the commit key and reads back as the manifest, the pack time and the covered position it was given.
- AC-2: With the node logging, a staged commit makes no write of its own; with nothing else logged, `tick` after two windows makes exactly one write for any number of shards waiting, and none before.
- AC-3: Only the newest logged commit of a shard stays unreleased: after three land, two have been released and the log is trimmed behind them when the register covers the newest.
- AC-4: A record older than the register's commit is released as it lands.
- AC-5: `named_within` and `named` never name a commit key, and admission of a commit key asks for the real shard's head.
- AC-6: With the setting off nothing is logged: the log's contents and request counts are what staging alone makes.
- AC-7: The janitor's reading of a dead log (`named`) names no commit key, so a log holding only commit records is collectable.
- AC-8: `OSS_COMMIT_LOGGED` without staging is refused; the setting is in the registry with default off.

## Test plan

`crates/writer/tests/it/log_commit.rs` for AC-1 to AC-5 over the committer rig (counting PUTs with the meter), a unit test for the record round trip, node role tests for AC-3 and AC-6, config and registry tests for AC-8; AC-7 follows from the `named` test, which the janitor reads through.

## S3 request budget

On: no request of its own while the node logs (the record joins an open window and adds its bytes, one manifest a shard waiting); one PUT per `tick` that finds records nothing carried, for all shards together. No GET, LIST or register write. Off: unchanged.

## Switch

`serverless.wal.node_log.commits.logged`, default `false`, with staging. It flips with the staging default after the cost soak (R11.5), when the reading half exists.

## Risks

A logged commit is a hint only: a successor that ignores it opens from the register. Bytes: one manifest a shard waiting per window, bounded by the log's window byte limit like any record; a manifest over the register's own bound is never staged.
