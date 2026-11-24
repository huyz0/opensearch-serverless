# 0049: A joining reader warms its cache from its peers

**Status:** Proposed; second independent review pending; 2026-11-24

## Context

A reader that joins serves its first searches from the object store: every page a query reads is a GET, for shards that were warm on another node a moment before. The shell fixes this with a warm copy: a joiner asks each search peer which of its readers the joiner will serve once ready, pulls the hottest blocks over the transport, turns ready at nine tenths, and reads the rest from the store.

A first draft of this ADR kept a table of ranged reads in the reader. An independent review showed that cannot work: the engine's store sees only page-aligned fetches on a miss (so every read count is about 1), a cache with no disk tier keeps nothing (`PageCacheConfig::default()`, which the node uses today), and a peer could not serve a page from its cache without fetching it on a miss. The cache itself has to say what it holds and how often it was read.

## Decision

1. **The cache is the hot set.** lucene-rust's `PageCache` gains, in its disk tier, a read count per page (up to 255), `resident_pages(prefix, limit)` (most read first), `peek_page(key, page)` (no read counted, no fetch) and `insert_page(key, page, bytes)` (lucene-rust PR 570). A file's cache key is the shard's namespace (`{base}/{index}/{shard}/t={term}`), the file name, its stored length and its layout, so it is the same on every node and a page of a blob (immutable) is the same bytes on every node.
2. **A node has a disk tier only when asked.** `OSS_PAGE_CACHE_DIR` and `OSS_PAGE_CACHE_BYTES` give the node's cache a disk tier; unset (the default) keeps today's cache, which holds nothing past a read. The warm copy needs it and is off without it. Its own measurement decides the default.
3. **The wire is in pages, and the joiner trusts none of it.** A peer names, per shard, `(key, page, len)` for its hottest pages, and serves `(key, page)` batches from `peek_page`: a page it no longer holds is skipped, never fetched, so a peer makes no object-store request for a joiner. `insert_page` validates nothing, so the joiner's node does, before each call: it opens the shard first (the reader's open already builds each file's key from its own namespace, version and term), and accepts a page only if its key is exactly one of the open commit's files' plain (`p`) keys, `page` is below that file's page count, `len` equals the stored range of that page, and the file's name is not pinned (the node parses the name out of the key; `segments*` and the pinned extensions are left to the opening reads). Decoded (`d`) files are not warmed: their page geometry needs the codec. A page that fails any check is dropped and counted; it does not count toward the ready fraction, so a peer on another term or version warms nothing and costs nothing. A joiner whose page size differs from its peer's refuses the peer. Warmed pages carry no read count (a second joiner sees no heat in them).
4. **Which shards.** A peer lists the shards it holds as a reader where a joiner, added to the live leases with the `search` role, lands among the first `width` readers (`readers_for`), `width` being the peer's own spread view of that shard, at most `max_shards` (default 1,000) and at most `max_bytes` of named pages (default 25% of the disk tier). The listing is one pass over the tier under one lock hold for all the shards asked (`resident_pages` takes the prefixes and a per-prefix limit), and a peer answers at most one listing at a time and reuses one for 5 seconds, so a serving peer's searches wait on it for a bounded time; placement is O(members) a shard, over at most `max_shards` shards.
5. **Authenticated, bounded and never blocking.** The actions ride the cluster transport and its shared-secret check (the one the tail actions use); a handler answers only for shards the peer holds and keys under their namespaces, whatever the request names. Request and answer are capped (`max_pages_per_request` 64, so under 4 MiB at the 64 KiB default page size); a peer serves at most `max_concurrent` (default 2) requests at once on a pool apart from search. A joiner reserves `batch bytes × parallel_peers` from its pool before asking, keeps to `max_bytes_per_second` (default 64 MiB/s), and skips a peer that errors, refuses or stalls; a failed warm-up leaves it serving cold, as before.
6. **Ready.** `Readiness` (ADR 0048's ready flag, PR 423) gains `Warming`: the node answers 503 on ready and stamps the lease without `search` until it holds `ready_fraction` (default 0.9) of the bytes its peers named (the total fixed when the lists arrive; a peer that fails shrinks both sides), or `max_wait` (default 60 s, at most 5 minutes) passes. With the switch off a node is ready at once.
7. **Switch.** `serverless.search.warm_on_join.enabled`, cluster default and overridable per index (`index.serverless.search.warm_on_join.enabled`: an index off is left out of the lists). Default off until measured: the joiner's object-store GETs over the whole warm-up and its time to first-query p50, and the peers' search p99, bytes and CPU while serving, with the copy on and off, on the emulator with latency, repeated runs with their spread. On only if the peers show no downside.

## Not measured

Everything in decision 7; and the cost of a disk tier to a node that does not warm (page write, `hits` update under the tier's lock).

## Consequences

- A lucene-rust pin bump to the revision of PR 570, in its own node PR; the disk tier ships as its own default-off switch, separately mergeable from the copy.
- Mixed fleets: a Java node answers the new actions with an unknown-action refusal, a peer skipped with a counted metric; Java joiners and peers warm among themselves. ADR 0001 keeps the shell frozen.
- A pinned file's pages are held whole and warmed by the opening reads, so the list excludes them (their hits are not read counts).

## Alternatives rejected

A reader-side table of ranged reads: the review showed it records page fetches, not reads. Naming only (the joiner fetches from the object store itself): none of the requests the copy exists to save are saved. Copying a peer's whole cache: pages of shards the joiner will not serve. Serving a peer's pages through its ordinary read path: a miss would cost the peer a GET.
