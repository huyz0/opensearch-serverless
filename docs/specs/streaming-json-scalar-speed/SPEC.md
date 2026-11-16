# Streaming JSON tokenizer: cheaper tokens in the fast loop

## Serves

The speed goal of [streaming-json](../streaming-json/SPEC.md): close the gap to serde_json's validate-only mode without buffering a document.

## Change

A profile of `feed` showed the gap to serde_json is per-token work in scalar code, not the byte scan: serde_json, also scalar, spends about 14 instructions a byte on `records` and 15 on `numbers`, `feed` spent 21 and 29. So this change makes the fast loop's tokens cheaper before any SIMD classification is tried.

- The fast loop's number scan follows the JSON number grammar directly, skipping digit runs eight bytes at a time (SWAR), instead of stepping the general number state machine a byte at a time. It returns `None` for anything it is unsure of (a number cut by the chunk end, or followed by a byte that could continue it), which the general machine then handles and reports, as before.
- A colon right after a key is consumed with the key.
- The escape-free string scan is always inlined into the fast loop.
- No `unsafe`, no new dependency.

Not in this change: SIMD structural classification, the serializer, an async adapter.

## Acceptance criteria

- AC-1: events and errors are unchanged: the push-against-pull and serde_json property tests of [streaming-json-feed](../streaming-json-feed/SPEC.md) pass unchanged.
- AC-2: the number scan returns the end of a number only when the number is complete and valid and the next byte cannot continue it, and `None` otherwise (unit tests, every byte value in every lane for the digit test).
- AC-3: on the bench host, `feed` in 64 KiB chunks is faster than before this change on `records` and `numbers` and no slower on `bulk` and `big_text` (`bench/`, provisional).

## Test plan

- AC-1: `tests/feed.rs`, `tests/oracle.rs`.
- AC-2: unit tests in `src/number.rs`.
- AC-3: `cargo run --release` in `crates/chunkjson/bench`, and instructions per byte under callgrind, which a shared host does not make noisy.

## S3 request budget

Unchanged: no S3 calls.

## Switch

None: same events, faster.

## Risks

- A fast-path number scan that accepts what the general machine refuses would change errors. It answers `None` on any byte that could continue a number, and the push-against-pull property test compares errors on arbitrary bytes.
