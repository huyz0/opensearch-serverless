# Reader: vector files warm at the first vector search

## Serves

The goals of fewer S3 requests, lower read latency and less memory for an index that holds vectors; the request budget's rule that an open reads what the first query needs, no more. Builds on [reader open prefetch](../reader-open-prefetch/SPEC.md).

## Change

The engine reads each vector file a search scans at random (`.vec`, `.vex`, quantized codes, IVF vectors) whole when a segment opens, if the file is under an eighth of the memory the process may use (`lucene_store::read_advice::warm_random`, on by default). On the object store that is a ranged read of every byte of every vector file, paid by the first search of the index even when it is a term search that reads no vector. Measured on the emulator and on RustFS 1.0.1 (docs/knn-measurements.md): an index of 100,000 vectors of 128 dimensions (47 MB) in 10 segments reads 47 MB in 61 range requests at open, whatever the first query is.

The reader now takes the warm out of the open and puts it at the first search that holds a `knn` query. That search hands the vector files of every segment to one prefetch before it walks any graph, so the files come in one parallel batch exactly as they did at open, and a search with no `knn` query never reads them. A file the open would not have warmed (too large next to the memory) is left out of the batch as it was left out of the open.

Not changed: the files the open reads whole (`open_prefetch`), the query path of every search without a `knn` query, the vectors a graph walk reads from a file too large to warm, and the answers.

## Acceptance criteria

- AC-1: with the warm at open (the old behaviour, `vector_warm::set_at_open(true)`), a cold open of the 10-segment vector index reads every vector file, as before.
- AC-2: with it off (the new default), a cold open and first term search of the same index read less than a quarter of the index's bytes, and no vector file.
- AC-3: with it off, a cold open and first `knn` search read the same bytes as the old open and first search (within 2 per cent), in no more requests and no more round trips, and a second `knn` search makes none.
- AC-4: the hits of a `knn` search, with their scores, are the same in both modes, and so are the hits of a term search.
- AC-5: a node setting `serverless.reader.vector_warm.at_open` (default false) sets the mode when the node starts; `true` is the old behaviour.
- AC-6: the batch is one `lucene_store::prefetch` over all segments; a prefetch that fails leaves the search to the engine's own reads, with the same answer.

## Test plan

- AC-1 to AC-4: `crates/reader/tests/it/vector_warm.rs`, over the emulator behind `Rounds`, a process of its own because the engine's switch is process-wide; seen red against the flag alone (the `knn` search read 136 ranges where the old open and search read 63).
- AC-5: `crates/node/src/vector_warm_setting.rs` unit tests and the boot config test.
- AC-6: reviewed in code: the batch's result is discarded; a refused range is retried by the page fault.
- Measurement: `crates/reader/tests/it/knn_requests.rs` on the emulator and on RustFS, recorded in `VERIFIED.md` and `docs/knn-measurements.md`.

## S3 request budget

Index with vectors, cold, first query a term search: bytes and requests fall to what the open and the term search read (measured below). First query a `knn` search: bytes the same as the old open plus search, requests no more, depth no more. LIST 0, HEAD 0, PUT 0. Idle: 0.

## Memory

The same bytes become resident at the first `knn` search instead of at open: the page cache's own accounting, unchanged. An index never searched by `knn` holds none of its vector files, where before it held up to an eighth of the memory per file. `open_cost` counts the files the engine reads whole into the heap; these files were never in that count (they are demand-filled).

## Switch

`serverless.reader.vector_warm.at_open`, node scope. Process-wide, because the engine's read advice is a process-wide table: an index cannot override it. The default is decided from the measurements in `VERIFIED.md`. R12.1 warm on join (`OSS_WARM_ON_JOIN`) copies the pages a peer holds, so a joining reader gets a vector file's pages from a peer only after a `knn` search warmed them there; the vector files of an index no one searched by `knn` are not copied, which is what a reader that has not served a `knn` search needs.

## Risks

A host without `userfaultfd` reads every file whole at open (there is no demand paging to defer), so the open reads the vector files there whatever this setting says. The first `knn` search of an index pays the warm that its open used to pay, so its latency moves from the open to the search; the open of a vector index is faster by the same amount.
