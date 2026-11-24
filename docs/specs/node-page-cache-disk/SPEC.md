# Node: an optional disk tier for the page cache

## Serves

ADR 0049 (a joining reader warms its cache from its peers), which needs a cache that keeps pages and counts their reads; parity task R12.1's first half.

## Change

`OSS_PAGE_CACHE_DIR` and `OSS_PAGE_CACHE_BYTES` give the node's page cache a disk tier: the pages a read fetched are kept under the directory, up to the byte budget, and counted per read. The directory is emptied at start; the tier is a cache and never outlives the node. Unset (the default) the cache is what it was: it keeps nothing past a read. A directory with no size, or a size under 1 MiB, refuses to start.

The tier's page records live in memory, one for every page its budget could hold, so the node reserves them from `open_indexes` at start and holds the reservation for its life; a node whose pool cannot hold them does not start.

Not here: listing pages to a peer, copying them, or readiness while warming (ADR 0049's later steps).

## Acceptance criteria

- AC-1: with no `OSS_PAGE_CACHE_DIR` the node has no disk tier; with one, a size of at least 1 MiB is required, and a missing, zero, too small or unreadable size refuses to start.
- AC-2: a node with the tier keeps the pages a search read (the directory holds bytes after it) and answers as a node without one does.
- AC-3: the tier's records are charged to `open_indexes` (one for every page the budget could hold) and released with the node; a pool that cannot hold them refuses to start; no tier charges nothing.

## Test plan

AC-1, AC-2: `crates/node/tests/server/boot_page_cache.rs`, over a booted node on the fs store. AC-3: the unit test in `crates/node/src/page_cache_disk.rs`, over a real budget.

## S3 request budget

None added. Fewer, later: a page kept is not fetched again. LIST 0, HEAD 0, PUT 0. A node that is never searched writes nothing under the directory.

## Memory

32 bytes of reservation for each 256 KiB page the budget could hold (8 KiB for 64 MiB; 128 MiB for a 1 TiB tier), from `open_indexes`, at start. The pages themselves are on disk, not in memory.

## Switch

`OSS_PAGE_CACHE_DIR` unset, which is the default: no tier. Whether it should default on is decided by the warm copy's measurement (ADR 0049), where it matters.
