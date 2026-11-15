# chunkjson

A sans-IO streaming JSON tokenizer. Feed it byte slices as they arrive and it returns one event per call; it never reads or blocks, so the same code runs in a blocking loop, a tokio task, or over a pooled buffer. Built for request bodies too large to hold whole: a `_bulk` body of any size, and single documents of hundreds of megabytes (Confluence pages, extracted PDF text).

Licence: Apache-2.0 ([LICENSE](LICENSE)). Version 0.1.0. It depends on no other crate of this repository, so it can move to its own.

## Owns
Tokenizing JSON from chunks: one document, whitespace-separated documents, or strict NDJSON, with strings streamed as fragments and each document's byte range reported.

## Depends on
Nothing in the workspace. `simdutf8` (pinned) validates string UTF-8.

## Entry points
`src/lib.rs` re-exports them:

- `Tokenizer::new(Config)`, then `next(input, eof)` returning `(Event, consumed)`. `NeedMore` means the slice is used up; pass `eof = true` when no bytes remain. After an error every call returns it again.
- `Event`: `BeginObject`, `EndObject`, `BeginArray`, `EndArray`, `Key(Text)`, `String(Text)`, `Number(&str)`, `Bool`, `Null`, `EndDocument { start, end }`, `NeedMore`, `Eof`. A `Text` is one fragment with `first`/`last` flags; unescaped runs are borrowed from the input, escapes are decoded.
- `Config { framing, max_depth, max_number_len }`; `Framing::{Single, Concatenated, Ndjson}`.
- `JsonReader::new(reader, buffer, config)` with `next_event()` over any `std::io::Read`, through a `Buffer` (implement it for a pooled buffer).

## Behaviour

- **Grammar:** RFC 8259, the same language serde_json accepts, checked by property tests and a fuzz target against it. Differences: a number outside f64's range is passed through as text (serde_json refuses it); nesting defaults to 128 (serde_json stops at 127). No comments, no trailing commas.
- **Memory:** the nesting stack and the number buffer are allocated once, by `Config`. A 64 MiB string fed in 64 KiB chunks grows the heap by nothing (`tests/memory.rs`).
- **NDJSON:** documents end at `\n`; `\r\n` and blank lines are allowed; a line break inside a document is `NewlineInDocument` and a second document on a line is `TrailingData`. Framing happens in the same scan as tokenizing.

## Speed (provisional)

`bench/` (outside the workspace) runs each contender in its own process; MB/s is input bytes over the median run, peak heap is growth during the parse. Measured 2026-11-15 on a shared 4-core cloud Xeon (AVX-512), so relative only. `ignored` contenders validate without building anything (serde's `IgnoredAny`); `value`/`tape`/`borrowed` build a document in memory.

| MB/s (peak heap MB) | bulk 21 MB NDJSON | records 10 MB | numbers 15 MB | big_text 135 MB, one doc |
|---|---|---|---|---|
| chunkjson, 64 KiB chunks | 696 (0) | 309 (0) | 236 (0) | 1,420 (0) |
| oss-rest tokenizer, 64 KiB | 366 (0.05) | 330 (0.05) | 259 (0.05) | 405 (0.05) |
| serde_json `IgnoredAny` | 1,042 | 1,166 | 549 | 2,358 |
| serde_json `Value` | 320 | 104 (68) | 231 (81) | 426 (349) |
| sonic-rs `IgnoredAny` | 983 | 1,220 | 853 | 2,410 |
| sonic-rs `Value` | 846 | 619 (126) | 355 (172) | 1,045 (1,215) |
| simd-json tape | 684 | 341 (97) | 256 (137) | 489 (498) |

The validate-only parsers need the whole document in memory and hand back nothing; the DOM parsers need several times the document. chunkjson streams with no growth and is fastest of the DOM-free streaming options on text, but loses 2 to 4 times to the validate-only parsers on structure- and number-heavy input: per-token overhead, which a SIMD structural scan is meant to remove.
