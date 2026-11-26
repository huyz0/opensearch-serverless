# Opening from a logged commit

## Serves

Parity plan R11.2, third slice, under [ADR 0064](../../adr/0064-commits-are-staged-and-checkpointed.md) point 6, after [logged-commits](../logged-commits/SPEC.md). Ported from the shell's `NewestCommits` and the takeover's use of it. With commit staging on, a node lost between checkpoints leaves a register up to a checkpoint age behind. A successor that opens from the newest logged commit instead replays only what that commit does not hold. A logged commit is a hint: ignoring it is always correct.

## Change

Revised after an independent review of the first draft (ordering, pack age and the swap's place were under-specified).

- `oss_writer::takeover::logged` reads, for a shard, the newest commit record under `<uuid>#commit` in each predecessor log the head names, and parses it (`CommitRecord::parse`). A record that does not parse, whose manifest does not parse or resolve, or that carries no `stage_seq`, is ignored (a record written by the Java shell is held to the same rule).
- **Which commit is newer** is decided by the manifest, never by log positions (those are one node's and not comparable across writers): a candidate is used only if its `(term, stage_seq)` is strictly greater than the register manifest's, read in the same register read whose generation the swap uses, and its term is at most the term the head was won at. Across several predecessor logs the greatest `(term, stage_seq)` wins. A full publish writes a `stage_seq` at least the staged ones it covers, so a register written after a logged commit is never regressed; the build checks that every publish carries one, and records it in the record if the register's manifest does not.
- **Pack age.** `oldest_pack_millis` is the predecessor's wall clock when its oldest pack was written; the successor reads its own wall clock. A candidate is usable while `now - oldest_pack_millis` is under `SWAP_DEADLINE_MILLIS - OPEN_MARGIN_MILLIS`, where the margin is a named constant that covers clock skew between nodes, the swap's latency and an upload's, with a compile-time assertion that it exceeds them. The age is checked at selection **and again immediately before the register is written**.
- **Where the commit is written.** The candidate is restored into the directory, becomes the publisher's last commit (generation, files, writer as read), and the replay's publish carries it: zero extra register writes when records were replayed. When none were, one bare swap, at the generation read, under the won term, after the second age check. A lost swap, or a restore or parse error, discards the directory and opens from the register alone, once (no loop).
- Behind `serverless.wal.node_log.commits.open_from_logged` (default `false`, `OSS_COMMIT_OPEN_FROM_LOGGED`, refused at node config validation without `logged`, as `logged` is without staging).

## Acceptance criteria

- AC-1: With a logged commit newer than the register (by `(term, stage_seq)`) in a dead predecessor's log, the replay is the records above the logged commit's sequence number, not above the register's.
- AC-2: The adopted commit is durable in the register before the predecessor's log is released; a lost swap leaves the head naming its writers and opens from the register alone.
- AC-3: Ignored, with the open the register alone gives: a commit not newer than the register (an older one at the same term after a full publish), a candidate with no `stage_seq`, a manifest that does not parse or resolve, a term above the head's, a pack older than the bound (tested both sides of it with a fake clock), a record written by the Java shell that does not parse. Across two writers' logs the greater `(term, stage_seq)` is used.
- AC-4: With the setting off, or no logged commit, the requests an open makes are unchanged (counted).
- AC-5: Opening from a logged commit makes no LIST and at most one ranged GET of the commit key's slice for each predecessor log, bounded by `MAX_NEWEST_BYTES`; the counts are the same at N and 10N records. Its row is in `docs/research/11-design/request-budgets.md`, and `scripts/check-budgets.py` holds it. With records replayed, no register write beyond the one publish; with none, one swap.
- AC-6: Crash test, the oracle being that every acknowledged id is found after takeover and the register names only files that resolve: a node lost before the swap, after the swap and before the head is settled, after it is settled, and with the predecessor still flushing its staged commit while the successor adopts. The test fails if the swap adopts a regressed manifest or one naming a deleted pack.
- AC-7: A returning predecessor: if its checkpoint swap lands first the successor's swap conflicts and opens from the register; if the successor's lands first the predecessor's flush is refused and it is fenced. Adoption does not stop a zombie from pushing commits to readers meanwhile (ADR 0064).
- AC-8: The setting is refused without `logged`, accepted with it, default off; both modes run in CI.

## Test plan

Writer tests over the committer rig and the takeover fixtures (fake clock for the pack age) for AC-1 to AC-3; node role tests with the helpers of `writer_takeover.rs` for AC-2, AC-6, AC-7; the meter for AC-4 and AC-5; config tests for AC-8; a Java-written record fixture for the cross-implementation case.

## S3 request budget

With records to replay: one ranged GET a predecessor log for the commit record, no register write beyond the replay's publish. With none: that GET and one swap. Off: unchanged. The commit GET adds one round between reading the head and restoring; judged against the round-trip budget in the build, which stays within it only if the read is issued with the head's log reads.

## Docs to update with the build

ADR 0064's consequence that a successor replays up to a checkpoint age more, the settings registry, the how-to for settings, the crate READMEs, the request-budgets row.

## Risks

The swap is the one point where a logged commit becomes register state; it is fenced by the register's compare-and-swap like any publish, the pack age is checked again just before it, and a manifest naming deleted packs is the bug the crash test fails on.
