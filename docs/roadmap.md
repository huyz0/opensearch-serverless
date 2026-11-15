# Roadmap

Built as a strangler beside the Java serverless shell, sharing its blob formats, one role at a time, by porting the Java ([porting.md](porting.md)) and proving OpenSearch compatibility as it goes ([compatibility.md](compatibility.md)). Each stage ends with a gate; a stage that fails its gate stops there and the Java role keeps serving. Basis: the assessment "lucene-rust, hsearch and pstore: what the serverless shell should do with them" (2026-11-14), section 3.

## 0. Foundation (this repository's first commit)

- Workspace, crate layout, gates, hooks, CI on plain Linux runners, agent instructions and skills.
- `oss-blob`: S3 request kinds, price classes, the meter.
- Decided: lucene-rust is the only code dependency, as pinned git crates; hsearch and pstore are references ([ADR 0002](adr/0002-runtime-dependencies.md)). Licence Apache-2.0 ([ADR 0003](adr/0003-apache-2-licence.md)).

## 1. Test harness and blob store (done 2026-11-15)

- **Memory foundation** ([ADR 0004](adr/0004-bounded-memory-and-streaming.md)): a node memory budget with named pools and reservations, a fixed-size buffer pool, and a test harness with a counting allocator that reports peak memory.

- An **in-process S3 emulator** with injectable latency, failures and request counting, streaming bodies through pooled buffers, so every test runs without Docker. RustFS as a plain binary stays optional for wire-level runs.
- The store trait in `oss-blob` and an S3 implementation, every call metered, reads and writes streamed in bounded chunks from the buffer pool (no whole-blob `Vec<u8>`), tested against the emulator and RustFS, with a peak-memory assertion.
- Request-count and round-trip-depth assertions as tests from the start (an idea from pstore).
- The store ports the shell's `MeteredBlobStore` and its blob-container use.
- The compatibility harness: fixture format, normaliser and runner, ready for the first endpoint.

Delivered: the memory budget, reservations and buffer pool (`oss-mem`); the counting allocator (`oss-testkit`); the blocking, metered, streaming store trait and in-process emulator (`oss-blob`, [ADR 0005](adr/0005-io-model.md), Proposed); `PooledStore`, which streams bodies through the node buffer pool and answers `Busy` when it is empty, with a test showing a 16 MiB object moves through one 64 KiB buffer (no extra heap on GET); and the compatibility harness with its first recorded fixtures (`oss-compat`).

Carried into stage 2: the real S3 client over SigV4 (ADR 0005), tested against the emulator and an optional RustFS binary; the bounded I/O pool, shared with lucene-rust's fetch executor.

## 2. Reader node

- Read the Java shell's published commits and segment blobs (one blob per segment, node packs) through lucene-rust's `CachedBlobDirectory` and page cache.
- Accept pushed tail records and search them through lucene-rust's realtime search.
- Answer `_search` for a declared DSL subset, porting OpenSearch's DSL parsing and query phase for it; the Java coordinator routes supported requests here and the rest to Java readers.
- A streaming REST body path for search requests and responses; tail records pushed over bounded channels.
- **Gate:** the compatibility suite passes for the subset (golden fixtures from OpenSearch), and the same answers as a Java reader on the shell's REST suites; idle index makes zero periodic requests (a test); heap per held shard measured.

## 3. Writer node

- Node write log with conditional-create fencing, segment build, one-blob-per-segment upload, commit publish and push, takeover.
- Runs in a mixed fleet with Java writers, writing the same formats.
- **Gate:** the shell's crash, takeover and fencing suites pass against the Rust writer; write p99 within 5%; no extra S3 requests.

## 4. Coordinator and REST

- REST, routing, fan-out and reduce, ported from OpenSearch and the shell. Java remains only for features not yet ported, then retires.
- **Gate:** the compatibility suite and OpenSearch's REST YAML suites for every in-scope endpoint, with skips listed.

## Alongside

- **Extensions:** C and WASM modules, with an ABI designed here and informed by hsearch's ABI v1, added when the reader needs its first one.
- **Cost guardrails:** per-request blob cost and freshness in responses, per-index request counters (an idea from pstore).
- **Cross-reference:** lessons found here go back to hsearch and pstore as notes or issues.
