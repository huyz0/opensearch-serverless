# Reader: open a frozen point-in-time shard

## Serves

R10.6: searching a point in time. The reader half: find a shard's frozen commit from a view and open it, never reading the shard's current manifest. Follows [point-in-time-capture](../point-in-time-capture/SPEC.md) and [reader-open-commit](../reader-open-commit/SPEC.md); ported from `ShardReaders.openFrozenReader`.

## Change

`ShardFiles::open_frozen(store, base, index, view, shard)` reads the index's descriptor (one GET, for the incarnation and the mapping) and resolves the files the view froze for `shard` from the view itself, so no commit register is read. `oss_reader::open_frozen_shard` opens it as `open_shard` does. A view is refused when it is of another incarnation of the name (`NotHeld`), still being captured or lacking the shard (`NotFrozen`), or records a file without a length or in a location no reader can follow.

Not here: forwarding a frozen search to the node that holds the view, the node's cache of open views, and closing them (node wiring, R7).

## Acceptance criteria

- AC-1: a view of the fixture's commit resolves to the same files, ranges and term as the live commit, with exactly one GET and nothing else.
- AC-2: the engine opens the frozen commit (5 documents) with one whole GET (the descriptor) and ranged reads only; the commit register is never read.
- AC-3: a view of another incarnation (uuid differs) is refused as `NotHeld`; one with no uuid is accepted by name.
- AC-4: a view that is still capturing, or has no entry for the shard, is refused as `NotFrozen`.
- AC-5: a file with no length is refused as `UnknownLength`; a location nobody can follow is a format error.
- AC-6: a missing index is not found after the one GET.
- AC-7: a view of a bucketed index resolves its files after the one descriptor GET, node-packed ones to their packs and others under its one shard's own path (`segments/<index>#<uuid>#0/`); an unfollowable location is refused.

## Tests

`crates/reader/tests/it/frozen.rs` over the shell fixtures in the emulator.

## S3 request budget

Open: 1 GET (descriptor), then ranged GETs as for any open. LIST 0, HEAD 0, PUT 0. The frozen manifest is not read.

## Risks

None beyond the live open's: a view keeps its blobs alive through the janitor's pins, so a held view's ranges exist.
- A frozen open and a live open of one term share a page-cache namespace (`base/index/shard/t=term`, as the live open always has). That is sound because a Lucene file name is never reused with other bytes within a term, and the namespace carries no uuid for the live open either. `a_frozen_open_costs_what_a_second_live_open_does_on_a_shared_cache` holds that a frozen open reads what a live reopen does, less the register.
