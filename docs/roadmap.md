# Roadmap

Built as a strangler beside the Java serverless shell, sharing its blob formats, one role at a time. Each stage ends with a gate; a stage that fails its gate stops there and the Java role keeps serving. Basis: the assessment "lucene-rust, hsearch and pstore: what the serverless shell should do with them" (2026-11-14), section 3.

## 0. Foundation (this repository's first commit)

- Workspace, crate layout, gates, hooks, CI on plain Linux runners, agent instructions and skills.
- `oss-blob`: S3 request kinds, price classes, the meter.
- **Open decisions:** how hsearch, lucene-rust and pstore are consumed ([ADR 0002](adr/0002-runtime-dependencies.md)); the licence.

## 1. Test harness and blob store

- An **in-process S3 emulator** with injectable latency, failures and request counting, so every test runs without Docker. RustFS as a plain binary stays optional for wire-level runs.
- The store trait in `oss-blob` and an S3 implementation, every call metered, tested against the emulator and RustFS.
- Request-count and round-trip-depth assertions as tests from the start (pstore's habit).

## 2. Reader node

- Read the Java shell's published commits and segment blobs (one blob per segment, node packs) through lucene-rust's `CachedBlobDirectory` and page cache.
- Accept pushed tail records and search them through lucene-rust's realtime search.
- Answer `_search` for a declared DSL subset; the Java coordinator routes supported requests here and the rest to Java readers.
- **Gate:** the same answers as a Java reader on the shell's REST suites for the subset; idle index makes zero periodic requests (a test); heap per held shard measured.

## 3. Writer node

- Node write log with conditional-create fencing, segment build, one-blob-per-segment upload, commit publish and push, takeover.
- Runs in a mixed fleet with Java writers, writing the same formats.
- **Gate:** the shell's crash, takeover and fencing suites pass against the Rust writer; write p99 within 5%; no extra S3 requests.

## 4. Coordinator and REST

- REST, routing, fan-out and reduce in Rust. Java remains only for features not yet ported, then retires.

## Alongside

- **Extensions:** C and WASM modules on hsearch's existing extension ABI v1, added when the reader needs its first one.
- **Cost guardrails:** per-request blob cost and freshness in responses, per-index request counters (from pstore).
