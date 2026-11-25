# Resident page bound

## Serves

[Search slots](../search-slots/SPEC.md): its cold-cache measurement (NOT-RUN in VERIFIED.md) needs searches that go back to the store, which needs a bound on the pages a reader keeps in memory. Also the memory-budget goal: reader memory is bounded by a setting, not by file size.

## Change

lucene-rust already bounds the pages a reader keeps mapped: its pager (`lucene_store::paged_input::Pager`) drops resident pages with `MADV_DONTNEED` past one process-wide budget (`set_budget`, default 1 GiB, 256 KiB pages here), and the next touch faults the page in again from the page cache's disk tier or the store. Nothing in this node ever set that budget, and `OSS_PAGE_CACHE_BYTES` bounds only the disk tier. So no lucene-rust change is needed; this spec adds the node's setting for the existing bound.

- `OSS_RESIDENT_PAGE_BYTES` (node scope, read at boot): the most bytes of demand-filled pages the node keeps resident across every open file. Applied to the process's pager when the page cache is built, which every node that assembles services does; the over-pool refusal happens there too.
- `0` (the default) leaves the engine's default budget, 1 GiB, as it is today. A value other than 0 must be at least 1 MiB and at most the `page_cache` memory pool; otherwise the node refuses to start.
- The usage route reports `resident_pages` (budget, resident bytes, faults, refaults, evictions) when the setting is not 0 and the pager is available.

Not here: a per-index bound (the pager is process-wide), charging resident pages to the memory pool as they fill, and a default other than the engine's. Where `userfaultfd` is unavailable files are read whole and the setting has no effect; the report is then absent.

## Acceptance criteria

- AC-1: the setting defaults to 0 (nothing applied, no report) and refuses text, a value under 1 MiB (other than 0) and a value over the `page_cache` pool.
- AC-2: with a value set, the pager's budget is that value after boot.
- AC-3: with a budget far below an index's pages, repeated searches hold resident bytes at or under the budget (plus one eviction batch) and the pager's evictions and refaults grow; with the setting 0 they do not.
- AC-4: searches answer identically with and without the bound.
- AC-5: the usage route reports `resident_pages` when set and not otherwise.

## Test plan

AC-1: config unit tests in `crates/node/src/boot/config/`. AC-2 to AC-5: `crates/node/tests/resident_pages.rs` over a booted node on the fs store; each seen failing before the setting is read. The cold-cache measurement is hand-run evidence in VERIFIED.md.

## S3 request budget

None added. A page dropped is read again from the disk tier if it holds it, else from the store: more GETs only for a node that sets a bound below its working set, which is what the bound is for. LIST 0, HEAD 0, PUT 0; idle indices make none.

## Memory

Bounds, rather than adds: resident decoded pages at the setting, instead of the file size up to 1 GiB.

## Switch

`OSS_RESIDENT_PAGE_BYTES`, default 0 (the engine's 1 GiB, as today). The default stays until the search p99 and request counts at a bound below the working set are measured (VERIFIED.md).

## Risks

A bound below the working set turns memory hits into store reads and raises p99; that is the setting's cost and why it is not on by default. The pager's budget is process-wide: two nodes in one process (tests) share it.
