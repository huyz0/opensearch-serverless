# Streaming JSON tokenizer: push API and fast path

## Serves

[ADR 0004](../../adr/0004-bounded-memory-and-streaming.md), and the speed goal of [streaming-json](../streaming-json/SPEC.md): close the gap to serde_json's validate-only mode without buffering a document.

## Change

- `Tokenizer::feed(chunk, eof, sink)`: tokenizes a whole chunk per call into a `Sink`. It checks the chunk's UTF-8 once, in one SIMD pass, instead of once per string, and handles the common tokens inside a document (punctuation, escape-free strings, numbers and literals that end inside the chunk) in a fast loop. Everything else falls through to the existing state machine, so events and errors are unchanged.
- Hot tokenizer functions are marked `#[inline]`.
- No `unsafe`: validated spans are borrowed with `str::get`, which checks character boundaries.

Not in this change: SIMD structural classification, the serializer, an async adapter.

## Acceptance criteria

- AC-1: for any JSON value and any chunking, `feed` rebuilds the value serde_json parses (property test).
- AC-2: for mutated JSON and arbitrary byte strings at any chunking, `feed` gives the same events and the same error (kind and offset) as `next`, and accepts exactly what serde_json accepts (property tests).
- AC-3: invalid UTF-8 in a chunk, including a character split across chunks, is reported where `next` reports it.
- AC-4: a sink error stops the feed and is returned.
- AC-5: on the bench host, `feed` in 64 KiB chunks is faster than `next` on every bench case (`bench/`, provisional).

## Test plan

- AC-1, AC-2: `tests/oracle.rs` and `tests/feed.rs`; a planted wrong literal in the fast loop and a planted wrong span both fail them.
- AC-3, AC-4: unit tests in `tests/feed.rs`.
- AC-5: `cargo run --release` in `crates/chunkjson/bench`.

## S3 request budget

Unchanged: no S3 calls.

## Switch

None: a new API beside the old one, which is unchanged.

## Risks

- The fast loop and the state machine could drift. The push-equals-pull property tests are the guard; any new fast case needs a planted-bug check against them.
