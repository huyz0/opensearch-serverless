# A node collects the write logs of the dead nodes it reaps

## Serves

R8 janitor: a dead node's write log is deleted once nothing in it is unpublished ([seams](../node-dead-logs/SPEC.md)); this schedules that. Proactive takeover (who takes a dead node's shards) is a separate design and not touched.

## Change

- `DeadLogPass` runs the janitor's `Collector` for each dead member that still names log epochs. Those members come from the membership refresh the node already makes (`Refreshed::dead_with_epochs`), so finding them costs no request.
- Only a dead node's reapers collect it: the two live ingest nodes that rank highest by rendezvous on the dead node's id (`reapers_for`), the same two on every node. Any other node makes no request.
- A log that cannot be collected yet (`StillNeeded`), would be collected by a dry run, or failed, is not tried again at once: the wait starts at one pass interval and doubles up to 64 times that. A log nothing takes over therefore costs a bounded number of requests an hour, not one pass's worth every interval.
- The lease renewal posts each refresh's snapshot to a one-slot mailbox and never waits on a collection; a thread of its own (`dead-log-pass`, every `janitor_every`) takes the latest and runs the pass. The pass reserves its buffer from the request-body pool.
- Dry run follows the node's `sweep.dry_run`, on by default.

## Acceptance criteria

- AC-1: a node that is not a reaper of a dead node makes no request for it.
- AC-2: a reaper whose log is still needed does not try it again until its wait has passed, and the wait doubles; a waiting log costs no request.
- AC-3: once a successor has published everything the log held, a dry run reports it collectable and a real pass collects it, with no LIST.
- AC-4: the mailbox hands the pass the latest snapshot, once.
- AC-5: a booted node reaps the log of a node that died: with `OSS_JANITOR_EVERY_MS` short, its pass fences the dead node's log once the lease has lapsed.

## Test plan

`crates/node/tests/boot.rs` (AC-5, over a file store and real sockets) and `crates/node/tests/dead_log_pass.rs`: two nodes over one emulator, the first dead by a clock an hour on; the request meter counts what each pass spends.

## S3 request budget

Per dead node a reaper tries: the lease read, the log's fence reads and puts, a head read and a commit read per shard the log names, and the deletes. Per dead node a non-reaper: none. Retries back off to once in 64 intervals; no LIST.

## Risks

- A dry run still fences the dead node's log (the collector fences before it judges), as the shell does; the node is already judged dead by its lease.
- A snapshot taken at one refresh can be stale by one renewal interval; the collector reads the lease again before it acts.
