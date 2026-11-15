# Streaming JSON tokenizer (scalar stage)

## Serves

[ADR 0004](../../adr/0004-bounded-memory-and-streaming.md) (stream end to end, bulk bodies through a streaming parser), for documents too large to buffer whole: Confluence pages and extracted PDF text, where one `_source` can be many megabytes.

## Change

- A new crate, `crates/chunkjson`, with no dependency on any workspace crate, its own README, licence and version, so it can move to its own repository with `git filter-repo` unchanged.
- A sans-IO tokenizer: the caller feeds a byte slice and gets one event back, or `NeedMore` once the slice is used up. It never blocks and never reads, so it runs as is inside a tokio task; each call does work bounded by the slice it is given.
- One document streams: a string value comes out as a run of fragments, so a 100 MB string never sits in memory. Only numbers are buffered, up to a configured length.
- Three framings in the same scan: one document, whitespace-separated documents, and strict NDJSON (one document per line). Each document's end event carries its start and end byte offsets, so a caller can copy the raw bytes of `_source` without re-encoding.
- A blocking `Read` adapter over a caller-supplied buffer (the crate's own `Buffer` trait, which a node's pooled buffer implements).
- A benchmark package beside the crate, outside the workspace, comparing it with serde_json, sonic-rs and simd-json, including one very large document.

Not in this change: the SIMD structural scan, the serializer, an `AsyncRead` adapter, wiring into `_bulk`. Each lands only if the benchmark backs it.

## Acceptance criteria

- AC-1: for any JSON value, serialised compact or pretty and fed in chunks of any sizes (including 1 byte), the events rebuild a value equal to `serde_json::from_slice` of the same bytes (property test).
- AC-2: for byte strings mutated from valid JSON, the tokenizer accepts exactly when serde_json accepts, except numbers outside f64's range, which the tokenizer passes through as text and serde_json refuses (property test).
- AC-2a: in concatenated framing, the documents equal serde_json's `StreamDeserializer` output for the same mutated, randomly chunked bytes, and adjacent scalars such as `1true` are refused with `TrailingData` (property test).
- AC-3: lone and misordered UTF-16 surrogate escapes, invalid UTF-8 (including a multi-byte character split across chunks and then broken), raw control characters in strings, bad escapes, bad numbers and trailing data are refused with a typed error and the absolute byte offset of the fault.
- AC-4: nesting of `max_depth` (default 128; serde_json stops at 127) is accepted and one more is refused with `DepthExceeded`; a number longer than `max_number_len` is refused with `NumberTooLong`.
- AC-5: a 64 MiB string value fed in 64 KiB chunks is tokenized with no fragment longer than the chunk and no heap growth above 64 KiB beyond the tokenizer's construction, asserted by a counting allocator.
- AC-6: in NDJSON mode, documents separated by `\n` (with `\r\n` and blank lines allowed) each end with their byte offsets, a newline inside a document is refused with `NewlineInDocument`, and two documents on one line are refused with `TrailingData`.
- AC-7: the `Read` adapter gives the same events as the slice API for every chunk size, and passes through I/O errors.
- AC-8: after an error, every later call returns the same error.
- AC-9: no manifest under `crates/chunkjson`, including `bench/` and `fuzz/`, names a workspace crate or a path outside it, in any section (`scripts/check-layering.py`).

## Test plan

- AC-1, AC-2: proptest against serde_json as the oracle, split at random points; they fail on any state lost across a chunk boundary.
- AC-3, AC-4, AC-6, AC-8: unit tests per error kind, at chunk size 1 and whole; each asserts the kind and offset.
- AC-5: an integration test binary with its own counting allocator.
- AC-7: the property inputs replayed through a `Read` that returns at most N bytes per call.

## S3 request budget

Unchanged: no S3 calls.

## Switch

None: a new library nothing uses yet.

## Risks

- A grammar difference from serde_json that the property generator never reaches. A cargo-fuzz target runs the same oracle on arbitrary bytes.
- Scalar speed may lose to the DOM parsers on small documents; the benchmark says by how much, and the SIMD stage has to close it.
