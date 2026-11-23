# Janitor sweep scope: judge a shard's older terms without listing them

## Serves

R8.4, invariants I-1 (every call metered, and the fewest the rule allows) and the fencing rule. Ported from `reconcile/SweepScope` at huyz0/OpenSearch `3920dc36`. Follows [janitor-gc](../janitor-gc/SPEC.md), which lists the shard's whole storage path on every sweep; a sweep is a LIST, priced like a PUT, for every publish and for every sweep of a blob being watched.

## Change

- `gc::Memory` also remembers, for each shard it has swept, a scope: the live term the sweep judged under, when the shard was last listed (the sweep's clock), and the older-term blobs the manifest or a pin named plus those the sweep was watching. Nothing in a node reads it but the collector.
- `gc::Config::relist_after_millis`: how long a sweep may judge from the scope before it lists again. 0 lists every sweep (the previous behaviour). The node passes one hour, the Java's `SweepScope.RELIST_AFTER_MILLIS`.
- A sweep lists when the shard has no scope, the scope was taken under another term, `relist_after_millis` is 0, or the last listing is that old (or in the future by the sweep's clock). Otherwise it judges the scope's blobs and the watched ones under exactly the rule of the listing sweep: older term, not named by the manifest or a pin, seen unreferenced by an earlier sweep and for the minimum age. It then keeps a new scope of what the manifest and pins name in older terms and what it still watches, with the listing time unchanged.
- `Sweep::listed` tells whether the sweep listed.

What the scope cannot see is a blob nothing named since the last listing: a fenced zombie's late write into an older term container. The hourly relisting finds it, which bounds how long it waits; the Java makes the same trade. No safety rule is weaker: a blob is deleted only by the three rules of janitor-gc.

No switch: the sweep deletes what a listing sweep would have judged worth deleting, with fewer requests, and the Java does the same.

## Acceptance criteria

- AC-1: with `relist_after_millis` 0 every sweep lists (the request counts of [janitor-gc](../janitor-gc/SPEC.md) hold).
- AC-2: with a relist interval, the first sweep lists and a later one inside the interval and the same term does not: no LIST, and a blob the manifest stopped naming is still deleted on the second sweep that sees it unreferenced.
- AC-3: a blob the manifest still names is never deleted by an unlisted sweep, and one a pin names is not either.
- AC-4: a new live term lists again, and so does a sweep after the interval; the blob nothing named (the zombie's) is found by that listing and deleted by the next sweep.
- AC-5: a dry run reports the same would-delete set unlisted as listed.
- AC-6: a sweep that cannot read the pins or the manifest leaves the scope as it was.

## Tests

`crates/janitor/tests/gc_scope.rs` on the emulator with its meter; the existing `gc.rs`, `point_in_time_pins.rs` and `crates/node/tests/writer_janitor.rs` run with the interval at 0.

## S3 request budget

LIST per sweep falls from one (a page a thousand keys) to one an hour per shard plus one at each new term. The register GET and the delete batch are unchanged. 0 requests for an idle shard (the pass does not sweep it).

## Risks

- A zombie's leftovers wait up to an hour longer for collection; storage only, never a record.
- The scope holds one key per older-term blob the manifest names: bounded by the manifest's file limit, and reserved with the pass's pool reservation like the watched set.
