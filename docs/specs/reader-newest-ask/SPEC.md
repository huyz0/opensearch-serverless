# The reader asks a staging writer for its newest commit on a cold open

## Serves

Parity plan R7.6, the last of the push carry-overs; [writer-newest](../writer-newest/SPEC.md) answers the ask. The asking side of the shell's `NewestCommits.newerThan`: a writer that stages commits writes the register only as a checkpoint, so a register that carries a stage sequence can be minutes behind. A reader opening cold asks that writer for its newest commit and takes it only if it is a later stage of the same writer and term.

## Change

- `oss-reader` gains `commit_push::NewestSource` (what asks a writer) and `commit_push::newer_commit`, the sans-I/O decision over register values, and `ShardFiles::open_asking_with_meta`, which opens on the newer of the register's commit and the writer's answer. Only a register read is made: no request is added.
- The ask is made only when the register's commit has a stage sequence and a writer. A commit the writer staged later (same writer, same term, higher sequence) replaces it; anything else, and any failed or empty ask, leaves the register's commit: older, never wrong.
- `oss-node` gains `newest_ask::TransportNewestAsk`, a `NewestSource` over the transport client: the writer must be live in the leases the node holds (no lease, an expired or a revoked one is never asked, and no request is made to the store to find out); the call is signed, goes to `internal:serverless/commit/newest` and waits at most `ASK_TIMEOUT` (500 ms); an answer that does not decode is no answer.
- `SearchService::set_newest_source` installs it, and `newest_ask::install(service, enabled, source)` installs it only when the setting is on; a cold open then uses it. Not installed, a cold open is as before.
- Setting `serverless.reader.newest_commit_ask` (node scope, default `false`): whether node assembly calls `install` with it on. **Off by default** because the ask runs inside the cold open, after the register GETs and while the search holds the open's lead, so a live but frozen writer costs every cold open of a staged-register index up to the 500 ms bound (`a_source_that_takes_the_whole_timeout_costs_a_cold_open_that_long_and_no_more` shows the bound, not a measured cold-open latency). It adds no store request and a register with no stage sequence never asks. It goes on by default when a cold-open latency measurement with it on is recorded here.
- Known divergence from the Java: a reader on the writer's own node has no local branch (the shell answers from its own reconciler), so with no `LiveWriters` entry for itself it serves the register. Nothing in this build runs a reader and a writer in one process yet.
- Not changed: node assembly calling `install` with the setting's value and a `LiveWriters` over the membership, the tail overlay.

## Acceptance criteria

- AC-1: a register commit staged by writer `w` at sequence 4 and a newest commit staged by `w` at sequence 6 in the same term: the newer is chosen; a different writer, a lower or equal sequence, another term, or bytes that are no commit: the register's.
- AC-2: a register commit with no stage sequence, or no writer, is never asked about.
- AC-3: a cold open with a source whose answer is a later stage sees the documents of that commit, with the same GETs as without a source.
- AC-4: a source that answers nothing, or an older commit, leaves the open on the register.
- AC-5: the transport source asks a live writer once, signed, and decodes its answer; a writer with no lease, an expired lease or a revoked one is not asked; a writer that does not answer within the timeout, answers a failure or answers garbage is no answer.
- AC-6: the setting exists, is node scope and defaults to false; `install` with it off installs nothing and with it on installs once.
- AC-7: a source that takes the whole timeout costs one cold open that long, the search still answers the register's documents, and the open index then answers with no further ask.

## Test plan

`crates/reader/tests/commit_push.rs` (AC-1, AC-2), `crates/node/tests/newest_ask.rs` (AC-3, AC-4, AC-6, AC-7 over a real writer's publishes) and `crates/node/tests/newest_ask_transport.rs` (AC-5 over a real socket with a guarded handler), `crates/node/src/settings.rs` (AC-6).

## S3 request budget

Unchanged: a cold open is the two register GETs; the ask is a transport call. Idle indexes make none.

## Bounds

One ask per cold open, at most 500 ms; the answer is at most `MAX_NEWEST_BYTES` of the writer's manifest and is dropped after the open.
