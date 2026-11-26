# Opening from a logged commit

## Serves

Parity plan R11.2, third slice, under [ADR 0064](../../adr/0064-commits-are-staged-and-checkpointed.md) point 6, after [logged-commits](../logged-commits/SPEC.md). Ported from the shell's `NewestCommits` and the takeover's use of it. With commit staging on, a node lost between checkpoints leaves a register up to a checkpoint age behind. A successor that opens from the newest logged commit instead replays only what that commit does not hold. A logged commit is a hint: ignoring it is always correct.

## Change

- `oss_writer::takeover::logged` reads, for a shard, the newest commit record under `<uuid>#commit` in each predecessor log the head names (`for_each_record_above` on the commit key, newest one kept) and parses it (`CommitRecord::parse`). A record that does not parse, or whose manifest does not parse, is ignored.
- A candidate is used only when all hold: its oldest pack is younger than the pack swap deadline (`SWAP_DEADLINE_MILLIS` less a margin) by the successor's clock, so the janitor cannot have removed the packs it names; its term is the register's term or newer, and, at the same term, its covered log position is not behind what the register's commit covers; it names a manifest the register does not already hold.
- The open restores the candidate's manifest (`restore_value`) instead of the register's commit, so `committed_through` is higher and `adopt` replays less.
- Before `adopt` settles the head (lets go of the predecessors' logs), the candidate is written into the register by one compare-and-swap at the generation read: the successor's term in the manifest, as the first publish of a term does. If the swap loses, the open restarts from the register (a hint never blocks an open).
- Behind `serverless.wal.node_log.commits.open_from_logged` (default `false`, `OSS_COMMIT_OPEN_FROM_LOGGED`, refused without `logged`).

## Acceptance criteria

- AC-1: With a staged logged commit newer than the register in a dead predecessor's log, the successor's replay is the records above the logged commit's sequence number, not above the register's.
- AC-2: The adopted commit is in the register (one swap) before the predecessor's log is released; a lost swap leaves the head naming its writers and the open starts again from the register.
- AC-3: A logged commit whose oldest pack is older than the swap deadline, whose manifest does not parse, or that is not newer than the register is ignored, and the open is the one the register alone gives.
- AC-4: With the setting off, or no logged commit, the requests an open makes are unchanged.
- AC-5: Opening from the logged commit makes no LIST and one GET of the commit key's slice a predecessor writer; no request scales with records.
- AC-6: Crash test: a node lost with staged, logged commits loses no acknowledged write, whichever of the logged commit or the register the successor opens from.
- AC-7: `OSS_COMMIT_OPEN_FROM_LOGGED` without logging is refused; the setting is in the registry, default off.

## Test plan

Writer tests over the committer rig and the takeover fixtures for AC-1 to AC-3 (a fake clock for the pack age), node role tests for AC-2, AC-4 and AC-6 (the takeover helpers of `writer_takeover.rs`), config tests for AC-7, request counts with the meter for AC-4 and AC-5.

## S3 request budget

On: per predecessor log, the reads the replay already makes find the commit key's slice in the headers the successor reads anyway; one extra ranged GET for the newest commit record, and one register swap per adopted commit (the first publish of the term already makes one). Off: unchanged.

## Risks

A manifest adopted from a log may name packs that are gone: the pack-age bound and a manifest parse and resolve are checked before the swap, and a restore error falls back to the register. The swap is the only point at which the log's commit becomes durable state: CAS on the register fences it as any publish.
