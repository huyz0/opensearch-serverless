# k-NN measurements: recall, graph-build memory, request count

Provisional: every number is from one 4-core cloud sandbox with the in-process emulator, not a production host. They are relative, not absolute. Vectors are indexed by the shard as a Rust writer indexes them (`m` 16, `ef_construction` 100, the plugin's defaults) and searched by the engine's `knn_top_hits` with the beam equal to `k`, as the `knn` query runs it.

Harnesses (ignored tests, run by hand, never part of `cargo test`):

- `crates/writer/tests/knn_measure.rs`: recall against exact search, ingest memory, search time.
- `crates/reader/tests/knn_requests.rs`: object-store requests of an open and a `knn` search.

```text
KNN_N=100000 KNN_DIM=128 KNN_SPACE=l2 KNN_DATA=clustered \
  cargo test -p oss-writer --release --test knn_measure -- --ignored --nocapture
KNN_DOCS=100000 KNN_DIM=128 KNN_SEGMENT=10000 \
  cargo test -p oss-reader --release --test knn_requests -- --ignored --nocapture
```

## Recall against exact search

Exact search ranks every vector by the space's score. Data is 100 Gaussian clusters ("clustered", a stand-in for embeddings) or independent Gaussians ("uniform", the hardest case for a graph). Queries are drawn from the same distribution. Segments are what the shard flushed, not merged.

| docs | dim | space | data | segments | recall@10 | recall@100 | mean search (k=10) |
|---|---|---|---|---|---|---|---|
| 20,000 | 128 | l2 | clustered | 3 | 1.000 | 1.000 | 0.15 ms |
| 100,000 | 128 | l2 | clustered | 14 | 0.994 | 0.999 | 0.6 ms |
| 100,000 | 128 | cosinesimil | clustered | 14 | 0.992 | 1.000 | 0.6 ms |
| 100,000 | 128 | innerproduct | clustered | 14 | 1.000 | 1.000 | 0.6 ms |
| 100,000 | 768 | cosinesimil | clustered | 70 | 0.980 to 0.992 | 0.9995 | 4 ms |
| 500,000 | 128 | l2 | clustered | 70 | 0.945 to 0.964 | 0.997 | 2.4 ms |
| 500,000 | 128 | cosinesimil | clustered | 70 | 0.968 | 0.997 | 2.6 ms |
| 500,000 | 128 | innerproduct | clustered | 70 | 0.998 | 1.000 | 2.1 ms |
| 100,000 | 128 | l2 | uniform | 15 | **0.62** | 0.83 | 0.9 ms |

Reading: on clustered data recall@10 is 0.94 to 1.0 and falls as segments multiply (each segment's graph is searched with beam `k`); on uniform 128-d data it is 0.62. That is the shape of Lucene's HNSW with no extra beam, which is what the stock plugin's `lucene` engine runs, so it is parity, not a regression; it was not compared against a stock node. Queries per row: 100 (50 or 20 for the largest, so 500,000 rows carry about ±0.02 of noise).

## Graph-build memory

Counting allocator over the whole ingest of one shard (documents parsed, buffered, flushed into segments held in an in-memory directory). The writer flushes by its indexing buffer, which counts vectors, so a segment's size is set by the buffer, not by the index.

| docs | dim | raw vectors | ingest peak above start | segments held after | transient above held |
|---|---|---|---|---|---|
| 100,000 | 128 | 49 MiB | 195 MiB | 154 MiB | 40 MiB |
| 100,000 | 768 | 293 MiB | 944 MiB | 905 MiB | 39 MiB |
| 500,000 | 128 | 244 MiB | 871 MiB | 773 MiB | 97 MiB |

The held figure is the in-memory directory, which a node does not keep (segments go to the object store). The transient figure, peak minus held, is the working memory of flushing and building: 39 to 97 MiB for segments of about 1,400 (768 dimensions) to 7,000 (128 dimensions) vectors. It is not yet checked against `oss-mem`'s write-buffers pool bound with the pool's own accounting: this measures process allocation, not a `Reservation`.

## Requests

`knn_requests` on an index of 100,000 documents of 128 dimensions (47 MB, ten segments) behind the emulator, a cold page cache for each row:

| step | term search | knn search |
|---|---|---|
| open the shard | Get=2, GetRange=61 | Get=2, GetRange=61 |
| first search | 0 | 0 |
| second search | 0 | 0 |

A `knn` search makes no request a term search does not, and no LIST. Here the open already read all 47 MB (the segment files are small enough for the page cache to read whole); on a host where a larger segment is demand-paged the first `knn` search may fetch vector pages. That case, and any number over RustFS, is **NOT-RUN**: running the RustFS binary in this sandbox is waiting on the owner's go-ahead.

## Verdict

- Requests: no change from a non-vector index in the case measured. RustFS and demand-paged vector files NOT-RUN.
- Memory: the flush working set above the held segments was 39 to 97 MiB a segment in the three runs; it was not measured against the node's pool.
- Recall: matches the stock algorithm; falls under 0.65 on uniform random vectors and near 0.95 at 500,000 unmerged clustered vectors.
