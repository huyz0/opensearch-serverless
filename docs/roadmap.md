# Roadmap

Built as a strangler beside the Java serverless shell, sharing its blob formats, one role at a time, by porting the Java ([porting.md](porting.md)) and proving OpenSearch compatibility as it goes ([compatibility.md](compatibility.md)). Each stage ends with a gate; a stage that fails its gate stops there and the Java role keeps serving. Basis: the assessment "lucene-rust, hsearch and pstore: what the serverless shell should do with them" (2026-11-14), section 3.

## 0. Foundation (this repository's first commit)

- Workspace, crate layout, gates, hooks, CI on plain Linux runners, agent instructions and skills.
- `oss-blob`: S3 request kinds, price classes, the meter.
- Decided: lucene-rust is the only sibling project used as code, as pinned git crates (other crates are the writing thread's choice); hsearch and pstore are references ([ADR 0002](adr/0002-runtime-dependencies.md)). Licence Apache-2.0 ([ADR 0003](adr/0003-apache-2-licence.md)).

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

### Stage 4, parity plan R7: status

Done and on main: routing, fan-out and reduce, document APIs, index admin, cluster and health REST, node assembly and roles, the one-node usable-index replay ([node-usable-cluster](specs/node-usable-cluster/SPEC.md)), forwarded reads ([node-read-forward](specs/node-read-forward/SPEC.md)), and the three-node REST suite ([node-cluster-suite](specs/node-cluster-suite/SPEC.md)). A top-level `top_hits` is merged and rendered in the search path ([coordinator-search-top-hits](specs/coordinator-search-top-hits/SPEC.md)).

R7 is complete: every item of the plan's R7 table is on main. Carried beyond it:

- `top_hits` inside a bucket aggregation: no reader serves it yet ([top-hits-agg](specs/top-hits-agg/SPEC.md)), so the reduce waits for that (R10).
- Fixtures: the bucket aggregations' expected bodies are read from the Java and not yet recorded against a stock node (see compatibility.md).
- The suite's RustFS run ([node-cluster-suite](specs/node-cluster-suite/SPEC.md) AC-5) has not been made with a binary.

## Parity plan R3 to R15: status (2026-11-26)

Counted by task from the plan "Rust serverless OpenSearch: plan to Java parity" (76 tasks), against `docs/specs/*/VERIFIED.md` and [compatibility](compatibility.md). **Done**: merged, with its acceptance evidence. **Built**: merged, but a named piece or proof is missing, or it is unwired or off by default pending evidence. **Left**: not built. Emulator and RustFS numbers are provisional.

| Milestone | Tasks | Done | Built | Left | Open |
|---|---|---|---|---|---|
| R3 storage and metadata | 7 | 7 | 0 | 0 | |
| R4 membership, placement, routing | 4 | 4 | 0 | 0 | |
| R5 node write log, fencing | 5 | 5 | 0 | 0 | |
| R6 indexing, mappings, publish | 6 | 6 | 0 | 0 | |
| R7 coordinator, usable cluster | 7 | 7 | 0 | 0 | |
| R8 takeover, janitor, reconcile | 7 | 6 | 1 | 0 | R8.6: no frozen-node suite, RustFS run only for takeover scale and the cluster suite |
| R9 freshness | 4 | 4 | 0 | 0 | |
| R10 search breadth, prefetch | 8 | 6 | 2 | 0 | R10.1: the `nested` query is served by the reader but no index can hold a nested mapping yet (writer slice left); R10.2: the `nested` aggregation is refused |
| R11 packs, log commits, buckets, schemas | 5 | 1 | 2 | 2 | R11.4 done: mappings are shared in memory by content and schema blobs are deliberately not stored ([ADR 0054](adr/0054-parsed-mappings-are-shared-by-content.md)); R11.1 packs are not wired into the node; R11.3 buckets are off by default; R11.2 log commits built and off by default (staging, logged commits, opening from them: [ADR 0064](adr/0064-commits-are-staged-and-checkpointed.md)); R11.5 cost soak measured, no default flipped ([commit-cost-soak](specs/commit-cost-soak/SPEC.md)) |
| R12 warm scale-out, roles, zones | 5 | 3 | 2 | 0 | R12.1 and R12.2: warm copy is off by default, its headline checks are NOT-RUN |
| R13 big writes, admission, guardrails | 5 | 5 | 0 | 0 | |
| R14 snapshots and the rest of the API | 8 | 7 | 0 | 1 | R14.6 runs no script; R14.8 stock restore and export done; restore in the background (off by default) done (the export's 403 on registering as a non-admin and a RustFS leg are NOT-RUN) |
| R15 k-NN, security, extensions, soak | 5 | 2 | 3 | 0 | R15.3 only the analyzer point; R15.4 proxy untried against live Java plugins; R15.5 no week-long soak, no Prometheus metrics |
| **Total** | **76** | **63** | **10** | **3** | |

63 of 76 tasks are done (83%); 73 of 76 are done or built (96%).

## Alongside

- **Extensions:** C and WASM modules, with an ABI designed here and informed by hsearch's ABI v1, added when the reader needs its first one.
- **Cost guardrails:** per-request blob cost and freshness in responses, per-index request counters (an idea from pstore).
- **Cross-reference:** lessons found here go back to hsearch and pstore as notes or issues.
