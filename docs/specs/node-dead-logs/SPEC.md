# The janitor's seams over a dead node's log and a shard's commit

## Serves

R8 janitor dead-node log collection ([collector](../../../crates/janitor/src/node_log/mod.rs)): the janitor does not depend on the writer, so the log and commit reads come in through seams.

## Change

- `oss_node::writer::StoreNodeLogs` implements `NodeLogs`: it fences the epochs a lease names (re-reading the lease as the takeover does), answers the shards, max sequence number and ends the fence found, and deletes a range of an epoch.
- `StoreCommits` implements `Commits` over `commit_checkpoint` ([checkpoint](../writer-commit-checkpoint/SPEC.md)).
- A writer's log header now names the index of each shard (the key's name, which does not take part in key equality), so a successor holding only a dead node's log can find the shard's descriptor and head.

## Acceptance criteria

- AC-1: a dead node's log whose head a successor took and whose writes the successor published is collected: a dry run says it would, a real pass deletes it and the lease forgets the epochs.
- AC-2: before the successor takes the head, the same pass says the log is still needed.
- AC-3: collecting issues no LIST.

## Test plan

`crates/node/tests/dead_logs.rs`: two nodes over one emulator, the first dead by a clock an hour on; checked with the real `Collector`.

## S3 request budget

Per dead node and pass: the lease read, the log's fence reads, a head and a commit read per named shard, and deletes of the log's blobs. No LIST.

## Risks

A log written before this change names no index; the collector treats that as still needed, so it is kept, never wrongly deleted.
