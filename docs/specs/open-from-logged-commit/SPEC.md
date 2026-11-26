# Opening from a logged commit

## Serves

Parity plan R11.2, third slice, under [ADR 0064](../../adr/0064-commits-are-staged-and-checkpointed.md) point 6, after [logged-commits](../logged-commits/SPEC.md). Ported from the shell's `NewestCommits` and the takeover's use of it. With commit staging on, a node lost between checkpoints leaves a register up to a checkpoint age behind. A successor that opens from the newest logged commit instead replays only what that commit does not hold. A logged commit is a hint: ignoring it is always correct.

## Change

Revised after an independent review of the first draft (ordering, pack age and the swap's place were under-specified).

- `oss_writer::takeover::logged` reads, for a shard, the newest commit record under `<uuid>#commit` in each predecessor log the head names, and parses it (`CommitRecord::parse`). A record that does not parse, whose manifest does not parse or resolve, or that carries no `stage_seq`, is ignored (a record written by the Java shell is held to the same rule).
- **Which commit is newer** is decided by the manifest, never by log positions (those are one node's and not comparable across writers). The takeover reads the register once; that read's manifest and generation are the ones compared against and the ones the publisher is handed (restore does not read it a second time). A candidate is used only if the register's `stage_seq` is at least 0, the candidate's `(term, stage_seq)` is strictly greater than the register's, its term is **strictly less** than the term the head was won at (an equal term is another writer's under `fence`), and its record states a known age (a record with no age is ignored). Across several predecessor logs the greatest `(term, stage_seq)` wins. A register with `stage_seq` -1 (staging off, or never staged) is never adopted over, because `stage_seq` is monotonic per publisher instance, not per register: a publisher made again in the same term, or staging switched off, restarts or clears it. For the same reason the publisher's `sequence` is seeded with the register's `stage_seq` when it reads or adopts one (max with its own), so the next logged commit exceeds what is in the register. An absent register (generation absent) refuses a candidate: opening from a log with no register is the replay already.
- **Pack age.** `oldest_pack_millis` is the predecessor's wall clock when the oldest pack its staged commits name was written. Soundness rests on the janitor collecting a blob only `COLLECTABLE_AFTER_MILLIS` (three swap deadlines) after it first saw it, not on skew being bounded: the build asserts at compile time only `OPEN_MARGIN_MILLIS < SWAP_DEADLINE_MILLIS` and `SWAP_DEADLINE_MILLIS + OPEN_MARGIN_MILLIS < COLLECTABLE_AFTER_MILLIS`. A candidate is usable while its age by the successor's wall clock is under `SWAP_DEADLINE_MILLIS - OPEN_MARGIN_MILLIS`. At selection that age becomes a monotonic `Instant` deadline carried with the adopted commit; the publisher applies `expired` to it exactly as it does to the packs it landed itself, before the replay's publish and before a bare swap, and returns `PackExpired`, which `adopt` treats as the fall-back-to-register case.
- **Where the commit is written.** The candidate is restored into the directory and seeds the publisher's last commit (generation as read, candidate files, candidate term and writer, the deadline above) through one `ShardPublisher` entry made for it; the replay's publish carries it: zero extra register writes when records were replayed. When none were, one bare swap at the generation read, under the won term, after the age check. **A lost swap is never retried by the publisher** (its retry-once on a conflict would publish the replay over a register that moved): a seeded publish returns the conflict to `adopt`, which leaves the head naming its writers; the node then discards the directory and opens from the register alone with a fresh restore, once, so there is no loop.
- Behind `serverless.wal.node_log.commits.open_from_logged` (default `false`, `OSS_COMMIT_OPEN_FROM_LOGGED`, refused at node config validation without `logged`, as `logged` is without staging).

## Acceptance criteria

- AC-1: With a logged commit newer than the register (by `(term, stage_seq)`) in a dead predecessor's log, the replay is the records above the logged commit's sequence number, not above the register's.
- AC-2: The adopted commit is durable in the register before the predecessor's log is released; a lost swap is not retried by the publisher, leaves the head naming its writers and opens from the register alone (a conflict injected between the register read and the swap leaves the register as the other writer wrote it).
- AC-3: Ignored, with the open the register alone gives: a commit not newer than the register (an older one at the same term after a full publish), a candidate with no `stage_seq`, a manifest that does not parse or resolve, a term above the head's, a pack older than the bound (both sides of it, with a fake clock, including a replay slow enough to cross it), a record with no age, a register whose `stage_seq` is -1 or whose publisher was made again in the same term (a stale logged commit does not win), a term equal to the head's, an absent register, a record written by the Java shell that does not parse. Across two writers' logs the greater `(term, stage_seq)` is used.
- AC-4: With the setting off, or no logged commit, the requests an open makes are unchanged (counted).
- AC-5: Opening from a logged commit makes no LIST and at most one ranged GET of the commit key's slice for each predecessor log, bounded by `MAX_NEWEST_BYTES`; the counts are the same at N and 10N records. Its row is in `docs/research/11-design/request-budgets.md`, and `scripts/check-budgets.py` holds it. With records replayed, no register write beyond the one publish; with none, one swap.
- AC-6: Crash test, the oracle being that every acknowledged id is found after takeover and the register names only files that resolve: a node lost before the swap, after the swap and before the head is settled, after it is settled, and with the predecessor still flushing its staged commit while the successor adopts. The test fails if the swap adopts a regressed manifest or one naming a deleted pack.
- AC-7: A returning predecessor: if its checkpoint swap lands first the successor's swap conflicts and opens from the register; if the successor's lands first the predecessor's flush is refused and it is fenced. Adoption does not stop a zombie from pushing commits to readers meanwhile (ADR 0064).
- AC-8: The setting is refused without `logged`, accepted with it, default off; both modes run in CI.

## Test plan

Writer tests over the committer rig and the takeover fixtures (fake clock for the pack age) for AC-1 to AC-3; node role tests with the helpers of `writer_takeover.rs` for AC-2, AC-6, AC-7; the meter for AC-4 and AC-5; config tests for AC-8; a Java-written record fixture for the cross-implementation case.

## S3 request budget

With records to replay: the commit record is read with the predecessor log's headers in `predecessors.of(writer)`, at most one ranged GET a log and no extra round, no register write beyond the replay's publish. With none: that GET and one swap. Off: unchanged. The commit GET adds one round between reading the head and restoring; judged against the round-trip budget in the build, which stays within it only if the read is issued with the head's log reads.

## Docs to update with the build

ADR 0064's consequence that a successor replays up to a checkpoint age more, the settings registry, the how-to for settings, the crate READMEs, the request-budgets row.

## Risks

The swap is the one point where a logged commit becomes register state; it is fenced by the register's compare-and-swap like any publish, the pack age is checked again just before it, and a manifest naming deleted packs is the bug the crash test fails on.
