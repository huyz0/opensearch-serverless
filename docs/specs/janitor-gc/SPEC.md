# Janitor: reclaim segment blobs no commit names

## Serves

Parity plan R8.4, first slice (the garbage collector), invariants I-1 (every store call metered) and the fencing rule. Ported from the shell's `reconcile/GarbageCollector.java` (`collectShard`'s graced sweep) at `3920dc36`. Decision record: [ADR 0020](../../adr/0020-janitor-crate.md).

## Change

- A new crate `oss-janitor` (layer 3, beside the reader and writer, never depending on either) holds the janitor's passes. This slice is its first: `gc::Collector::sweep`, one shard.
- **The rule, the Java's whole safety argument.** A blob is deleted only when it lives in a term container (`t=<term>/`) strictly older than the published manifest's term, **and** nothing names it: not the manifest (through any location form that points into the shard's path), and not a pin. Blobs outside `t=<n>/`, or of the live or a later term, are never judged.
- **The grace.** A blob must be seen unreferenced by two consecutive sweeps of the shard, and for at least `minimum_unreferenced_millis` (`serverless.janitor.minimum_unreferenced_millis`, node scope, dynamic, default 60,000 as the Java's node-pack collector; two back-to-back sweeps are no grace for a reader still serving the previous commit; 0 is a pure pass count, for tests) between the first sight and the delete. What a sweep saw is `gc::Memory`, held by the caller and keyed by the shard's storage path (so a new incarnation starts empty); a node that has just taken a shard starts empty and so deletes nothing on its first sweep. Forgetting only delays a deletion.
- **Pins.** `gc::Pins` answers, for one shard, the blobs a frozen view or a snapshot still names, or `Everything` when it cannot tell (an unreadable record, a capture still reading manifests). `Everything` deletes nothing and leaves the memory as it was, so the grace is not restarted. The crate ships no implementation that answers "no pins": point in time (R10.6) and snapshots (R14.2) wire theirs, and nothing can wire a sweep before (the same rule as `oss_meta::ShardPins`, ADR 0012). Tests carry their own.
- **No commit, nothing judged.** A shard with no manifest register may be mid-first-publish: every blob is a candidate for the commit about to be made. An unreadable or malformed manifest is an error, and nothing is deleted.
- **Dry run.** With `dry_run` on, a sweep does everything but the delete: it reports what it would delete, and keeps those blobs watched, so the sweep after the switch flips deletes them at once. `Config::max_watched` caps what a sweep holds and `Config::max_pinned` what the pins may name (the pass sets both from its pool reservation). The flip of `dry_run` also requires the minimum age to stay above the readers' refresh backstop. Default off since the fault suites and recorded mixed-fleet runs showed no live blob counted ([janitor-default-live](../janitor-default-live/SPEC.md)); `serverless.janitor.dry_run`, node scope. The setting is declared in `oss-janitor` and registered in the node's settings when the node wires a pass.
- Not in this change: the pass over every shard of every index, node packs and the log cleanups, point in time and snapshot pins, tombstone and reclaim scheduling, the node wiring.

## Acceptance criteria

- AC-1: a blob in a term older than the manifest's, named by nothing, is deleted by the second sweep that sees it and not the first.
- AC-2: a blob in an older term that the manifest names (an inherited file, whole or packed) is never deleted, over any number of sweeps.
- AC-3: a blob in the manifest's own term, or a later one, that nothing names is never deleted: a publish in flight.
- AC-4: a blob a pin names is never deleted; one that was a candidate and becomes pinned leaves the memory, and restarts its clock if it is unreferenced again.
- AC-5: `Pins::Everything` deletes nothing and leaves the memory unchanged.
- AC-6: a shard with no manifest register deletes nothing and lists nothing; a malformed manifest is an error and deletes nothing.
- AC-7: with a minimum age, a blob seen twice goes only once the clock has moved that far past its first sight.
- AC-8: a fresh `Memory` (a new owner) deletes nothing on its first sweep.
- AC-9: with `dry_run` on, nothing is deleted, no DELETE request is sent, the report names what would be, the blobs stay watched, and the first sweep after the switch flips deletes them.
- AC-10: keys outside `t=<n>/` (the manifest register, a non-numeric container, a nested key) are never judged.
- AC-11: a sweep deletes with `delete_many` and the request counts are exactly: one register read, one LIST page per 1,000 keys under the shard's path, one DELETE batch per 1,000 blobs deleted; no GET, HEAD or PUT.
- AC-12: the memory holds only candidates (what was deleted or referenced again leaves it); an empty memory holds no entry for the shard.
- AC-14: pins naming more than `max_pinned` keys are as if they said `Everything`.
- AC-15: a failed delete leaves the memory as it was, and the next sweep finishes the job.
- AC-16: a cap lowered between sweeps shrinks what is watched, but a blob already due is never held back by it.
- AC-13: a sweep watches at most `max_watched` blobs; the rest are judged by later sweeps.

## Test plan

`crates/janitor/tests/gc.rs` over the in-process emulator with its meter, writing manifests with `oss_format::manifest::encode` and blobs with `put` (AC-1 to AC-16). A test for each mutation of the rule: strict `<` of the term, the two-pass condition, the pin set, the reference set. The setting's default and bounds in `settings.rs`.

## S3 request budget

One sweep of one shard: 1 register read (GET-class), `ceil(keys / 1000)` LIST pages over the shard's storage path (LIST is priced like a PUT: the cost scales with the shard's blob count, as the Java's listing of its term containers does, and with nothing else; relisting avoidance is the next slice), and `ceil(deleted / 1000)` DELETE batches. Nothing scales with documents, indexes or elapsed time; an index is swept only when its trigger fires (a commit that dropped a file, a pin change), never on a timer, so an idle index makes none. The pass over shards lands with its own budget row.

## Switch

`serverless.janitor.dry_run`, default `false` (deletes), flipped from `true` on the evidence in [janitor-default-live](../janitor-default-live/SPEC.md). A deletion that nothing else would reclaim, with live blobs at stake, is a downside no measurement of the cost side can offset.

## Risks

- Deleting a blob a reader or snapshot still needs: the term and reference rules above, the two-sweep grace, and the dry run. Mutants on each rule are in the plan.
- A pin source that reads wrongly: the crate refuses to ship a "no pins" answer, so the first wiring is the real source.
- The listing's cost on a shard with very many blobs: bounded by the shard's blobs; relisting avoidance follows.
