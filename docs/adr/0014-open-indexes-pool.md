# 0014: Open indexes charged to a pool of their own, by an estimate made before opening, least recently used closed first

**Status:** Accepted after independent review, 2026-11-16.

## Context

A reader opens an index on its first search and keeps it open, so later searches make no request ([request budgets](../request-budgets.md): a warm search costs 0). What an open index holds in the heap stays until it is closed. Until now the reader capped how many indexes it keeps open (1,024) and closed an arbitrary one past that. A count says nothing about memory: measured on the emulator with indexes built by the engine (`oss_testkit::index`), one open index holds from 15 KB (one large compound segment, four fields) to 10.7 MB (20 segments of 100 text fields, written as separate files), so 1,024 of them may need 15 MB or 11 GB. The Java shell's `ShardReaders` keeps every reader it opens, with no bound but the JVM heap.

What the heap holds was measured, not guessed. lucene-rust's `CachedBlobDirectory` reads a file whole into the heap when its extension is pinned (`PageCache::is_pinned_name`: metadata, terms index, norms, stored-fields and points indexes), when it fits one page (256 KiB by default), or when the cache can neither demand-fill pages (`userfaultfd`) nor spill to its disk tier; other files are paged outside the heap. On top of the files read whole, the engine keeps about 11 KB a segment, about 2.2 KB a field a segment, and under 1 KB a file.

[ADR 0004](0004-bounded-memory-and-streaming.md) says memory is reserved from a pool before it is allocated, and a refused reservation answers 429.

## Decision

- **A sixth pool, `open_indexes`**, holds what open indexes keep in the heap. It is not `query_execution`, whose reservations last one request, nor `page_cache`, which is for the page cache's own pages: open indexes are long-lived, and one kind of memory must not starve another.
- **An index is charged before it is opened**, by an estimate made from the two register reads the open already makes (descriptor and commit manifest), so no request is added: a fixed 32 KiB for the shard, 16 KiB a segment (one per `.si` file), 3 KiB a field a segment (the mapping's fields plus 16 metadata fields), 1 KiB a file, plus every file the cache will read whole by the rule above, asked of the cache itself. Each constant is at least 1.35 times what was measured. A test opens indexes of 10 to 200,000 documents, 1 to 200 segments, compound and separate files, and up to 100 keyword or text fields, runs a mix of searches on each (a term query, sorts on a long and a keyword, a match query, hits fetched) so that what the engine loads on first use and keeps is counted, and checks that the estimate is never below the heap the index then retains, measured by the counting allocator, nor more than 8 times it or 128 KiB, whichever is larger. The searches added at most 7 KB to a small index and 13% to a large one; the closest estimate is 1.44 times the heap.
- **The charge lives as long as the open index**: it is released when the last holder (the map, or a search) lets go of it.
- **When the pool cannot take an index, the least recently searched index that no search holds is closed**, and the next, until the charge fits; if closing every such index could not make room, none is closed. An index a search holds is never closed: that would free nothing until the search ends and cost its next search a cold open. If none is left to close, the search answers 429 `circuit_breaking_exception` from the breaker `open_indexes`, durability `TRANSIENT`. An index larger than the whole pool closes nothing and answers the same with durability `PERMANENT`, as OpenSearch marks a request larger than a breaker's limit, so a client does not retry it. The cap on the count of open indexes goes.
- **First opens of one index are serialized**: the first search opens it, and searches of the same index wait for it rather than each charging the pool for a copy.
- Closing reads nothing and writes nothing; reopening costs the two GETs and ranged reads of a cold open again.

## Consequences

- A node's open indexes fit the bytes its budget gives the pool, whatever their shape; many small indexes and a few large ones are bounded alike. The pool's size comes from the node's budget split, as every pool's does; it starts at 0, and a reader refuses to start with an empty one rather than refuse every search. A setting and a default for it come before a reader binary ships (no binary starts a reader yet).
- No S3 request is added; an index evicted and searched again costs a cold open, as today past the count cap.
- The estimate is conservative on purpose: small indexes are charged about four times what they hold, so a pool holds fewer of them than would fit. Measuring an open's own heap is not possible without a counting allocator in production. A future lucene-rust accounting (`ramBytesUsed`) would replace it.
- The per-field cost is the scaling finding: 100 fields in 20 segments is about 4 MB an index before any query. Thousands of open many-field indexes on one node need merged segments, fewer fields or a larger pool.
- With a disk tier, the page cache pins a pinned file's pages on disk, and closing an index does not unpin them yet. That is disk, not heap, and no reader has a disk tier today; it needs handling when one is added.
- Least recently used needs a second ordered map beside the index map, updated on each search under the same lock: one entry moved a search, the two maps sharing one copy of the index name. Finding an index to close walks from the oldest past the held ones, at most the searches in flight.
