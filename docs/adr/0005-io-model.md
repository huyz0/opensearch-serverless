# 0005: Blocking object-store I/O, and a small S3 client of our own

**Status:** Proposed, 2026-11-14.

## Context

Every S3 request goes through `oss-blob` and is metered once per attempt, retries included (invariant I-3). Reads and writes stream through caller-provided fixed-size buffers so the buffer pool can own them ([ADR 0004](0004-bounded-memory-and-streaming.md)). Two choices follow: whether the store's API is blocking or `async`, and which S3 client sits under it.

Forces, with the numbers that exist:

- **The engine decodes memory, so it is blocking by design.** lucene-rust's codecs read plain `&[u8]` over files its pager fills on first touch (`crates/lucene-store/src/paged_input.rs`), and a memory access cannot `await`. Its `BlobStore` trait (`crates/lucene-store/src/blob_directory.rs`) and `CachedBlobDirectory` are synchronous for that reason. A query announces the ranges it will read first (`lucene_store::prefetch`, `Input::will_need`), and the page cache fetches them as one merged, parallel batch, so request count and round-trip depth are set by that batching, not by the I/O model. The assessment of 2026-11-15 rejected an async `Directory` and async query execution; the only part where async can help is the fetch layer under the page cache.
- **Concurrency per node is hundreds, not tens of thousands.** A query fans out one round of ranged GETs per shard it touches; a writer issues one log PUT per group commit. At 20 to 60 ms per S3 request (the shell's S3 latency profile, provisional), 256 threads in flight sustain roughly 4,000 to 12,000 requests a second per node, above the 5,500 GET/s S3 serves per prefix. Threads with 256 KiB stacks cost about 64 MiB of address space for 256 of them, and little resident memory.
- **Metering must see attempts, not calls.** A client that retries internally hides attempts unless it exposes a per-attempt hook.
- **Conditional create is load-bearing.** Fencing is `If-None-Match: *` on a PUT; the client must send it and report 412 (and 409 `ConditionalRequestConflict`) as a distinct outcome.
- **Builds stay small.** Every dependency is compiled by every gate run, and CI runs on free shared runners ([docs/ci.md](../ci.md)).

| Option | For | Against |
|---|---|---|
| **Blocking API; small S3 client over `ureq` with SigV4 written here** | Matches lucene-rust's trait directly; every attempt and retry is code we own, so metering is exact; streams into caller buffers through `std::io::Read`; few crates (hsearch's `crates/s3` takes the same route with `ureq` 3.5.2) | We write and test SigV4 (about 200 lines, checked against AWS's published signing test vectors) and the retry policy; one thread per in-flight request |
| `async` API on tokio with `aws-sdk-s3` | The official client; handles every S3 corner; scales to many thousands in flight per node | Colours every caller async while the engine is sync; a large dependency tree in every build; retries happen inside the SDK, so per-attempt metering needs its interceptor API; its body types make "stream into a caller buffer" indirect |
| `async` with `object_store` | One API over S3, GCS and Azure | Its own retries and request patterns are hidden; listing and multipart behaviour are chosen for us; same async colouring |
| `async` on tokio with hyper and our own client | Scales furthest; full control | All the cost of our own client plus an async runtime, for concurrency the node does not need yet |

## Decision

1. **The store API is blocking.** `oss-blob`'s `BlobStore` trait is synchronous and `Send + Sync`. Fan-out goes through one shared, bounded fetch executor per node, lucene-rust's range-fetch executor under its page cache, which takes a batch of ranges and bounds requests and bytes in flight for the whole node; its default backend is a thread pool, one thread per in-flight request. Nothing in `oss-blob` spawns threads.
2. **Buffers come from the caller.** A ranged GET fills a caller-provided `&mut [u8]` and hands each chunk to a sink; a PUT reads its body from a source into a caller-provided buffer. The buffer pool from ADR 0004 supplies these buffers; `oss-blob` allocates none per request.
3. **The S3 client is ours:** `ureq` (pinned at an exact version, rustls for TLS) for HTTP, AWS Signature V4 implemented in `oss-blob`, and a retry policy in `oss-blob` that records every attempt in the `Meter`. PUT bodies are signed `UNSIGNED-PAYLOAD` over TLS, so a body streams in one pass without hashing it first; a retry rewinds the source. It lands in its own change after the trait and the in-process emulator, and is tested against the emulator's semantics and, optionally, a RustFS binary.
4. **The emulator implements the same trait in-process** (Cargo feature `emulator`), so every test runs without Docker or a network.

## Consequences

- The reader plugs `oss-blob` into lucene-rust's synchronous `BlobStore` with no runtime bridge.
- Request counts are exact by construction: the code that sends a request is the code that records it.
- We own SigV4, retries, and the XML of `ListObjectsV2` and error bodies; a test suite against AWS's signing vectors and the emulator covers them.
- The request and result types (`ByteRange`, `GetOutcome`, `CreateOutcome`, `ObjectMeta`, `ListPage`, `BlobError`, `Meter`) carry no runtime assumption, so an async store trait, with async counterparts of `ChunkSink` and `ChunkSource`, can sit beside `BlobStore` and share them without changing existing callers.
- If a measured workload needs more than about 1,000 requests in flight on one node, or the fetch executor's queue wait shows up in search p99, this is revisited by giving the executor an async backend; callers, which hand it batches of ranges, do not change, and the trait's shape (caller buffers, sinks and sources) carries over to an async version.
- The REST server's I/O model is a separate decision; this one covers the object store only.
