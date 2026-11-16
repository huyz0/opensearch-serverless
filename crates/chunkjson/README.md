# chunkjson

A sans-IO streaming JSON tokenizer and writer. Feed it byte slices as they arrive and it returns one event per call; it never reads or blocks, so the same code runs in a blocking loop, a tokio task, or over a pooled buffer. Built for request bodies too large to hold whole: a `_bulk` body of any size, and single documents of hundreds of megabytes (Confluence pages, extracted PDF text).

Licence: Apache-2.0 ([LICENSE](LICENSE)). Version 0.1.0. It depends on no other crate of this repository, so it can move to its own.

## Owns
Tokenizing JSON from chunks: one document, whitespace-separated documents, or strict NDJSON, with strings streamed as fragments and each document's byte range reported. Writing compact JSON from the same events, strings fragment by fragment.

## Depends on
Nothing in the workspace. `simdutf8` (pinned) validates string UTF-8.

## Entry points
`src/lib.rs` re-exports them:

- `Tokenizer::new(Config)`, then `next(input, eof)` returning `(Event, consumed)`. `NeedMore` means the slice is used up; pass `eof = true` when no bytes remain. After an error every call returns it again.
- `Event`: `BeginObject`, `EndObject`, `BeginArray`, `EndArray`, `Key(Text)`, `String(Text)`, `Number(&str)`, `Bool`, `Null`, `EndDocument { start, end }`, `NeedMore`, `Eof`. A `Text` is one fragment with `first`/`last` flags; unescaped runs are borrowed from the input, escapes are decoded.
- `Tokenizer::feed(chunk, eof, &mut sink)`, the fast way: a whole chunk per call into a `Sink`, the same events and errors as `next`. It checks the chunk's UTF-8 once and handles common tokens in a fast loop. Returns `true` once the input is complete. After a sink error the tokenizer's offset is unspecified.
- `Config { framing, max_depth, max_number_len, comments, allow_empty }`; `Framing::{Single, Concatenated, Ndjson}`. `comments` accepts `//` and `/* */` comments where whitespace may go, and `allow_empty` lets single framing end with no document, as OpenSearch's Jackson parser does; both are off by default.
- Positions: `next_at` returns each event with its token's start as a `Position { offset, line, column }` (1-based, columns in bytes), a `Sink` gets it through `event_at`, and `Error::line()`/`column()` place a fault. `reset()` makes a tokenizer new again without allocating.
- `JsonReader::new(reader, buffer, config)` with `next_event()` over any `std::io::Read`, through a `Buffer` (implement it for a pooled buffer).
- `Writer::new(Config)`, then `event(Event, &mut out)` for tokenizer events, or the building calls `begin_object`, `end_object`, `begin_array`, `end_array`, `key`, `string`, `number` (JSON number text), `u64`, `i64`, `bool`, `null`; `check_complete()` says whether the output is complete. `out` is any `Output`: `Vec<u8>`, or `IoOutput` over a `std::io::Write`. A refused call returns `WriteError::Misuse` and writes nothing; after an output error every call is refused.

## Behaviour

- **Grammar:** RFC 8259, the same language serde_json accepts, checked against it by property tests and two fuzz targets, and by JSONTestSuite's 318 cases (`tests/jsontestsuite.rs`). Differences: a number outside f64's range is passed through as text (serde_json refuses it); nesting defaults to 128 (serde_json stops at 127). No comments unless `Config::comments`, no trailing commas.
- **Memory:** the nesting stack and the number buffer are allocated once, by `Config`. A 64 MiB string fed in 64 KiB chunks grows the heap by nothing (`tests/memory.rs`).
- **NDJSON:** documents end at `\n`; `\r\n` and blank lines are allowed; a line break inside a document is `NewlineInDocument` and a second document on a line is `TrailingData`. Framing happens in the same scan as tokenizing.
- **Writing:** compact output; strings escaped as Jackson's default generator does (`"`, `\`, `\b \f \n \r \t`, other control characters as `\u00XX` in upper case; `/` and non-ASCII as is). In NDJSON and concatenated framing a newline follows each top-level value. The writer never writes invalid JSON: it refuses the call instead. Its only allocation is the nesting stack, so a 128 MiB string written in fragments grows the heap by nothing (`tests/writer_memory.rs`). Not yet: pretty printing, float formatting.

## Speed (provisional)

`bench/` (outside the workspace) runs each contender in its own process on the same input, already in memory. MB/s is input bytes over the median of 11 runs (5 for big_text), the best of five runs of the harness made by hand, alternating contenders; peak heap is the allocator's growth during the run, with the input itself excluded for every contender. Measured 2026-11-16 on a shared 4-core cloud Xeon (AVX-512): relative only, and single runs vary by 10 to 30%.

Read the comparisons with these caveats:

- **Validate-only is not like for like.** serde's `IgnoredAny` (serde_json and sonic-rs) skips string content: serde_json's does not check its UTF-8, and neither decodes escapes. chunkjson checks UTF-8 and decodes escapes on every string.
- **NDJSON is split for the others.** The harness splits bulk bodies into lines and parses each line separately for every contender except chunkjson, which reads the framing itself. Part of chunkjson's lead on bulk is that split.
- **Building a tree is a different job.** `Value` and tape contenders build a document in memory; chunkjson only hands out events. Against them chunkjson wins by not building anything, not by parsing faster.

| MB/s (peak heap MB) | bulk 21 MB NDJSON | records 10 MB | numbers 15 MB | big_text 135 MB, one doc |
|---|---|---|---|---|
| chunkjson `feed`, 64 KiB chunks | 1,534 (0) | 945 (0) | 633 (0) | 2,115 (0) |
| chunkjson `next`, 64 KiB chunks | 739 (0) | 305 (0) | 235 (0) | 1,553 (0) |
| serde_json `IgnoredAny` | 1,241 | 1,186 | 640 | 2,886 |
| sonic-rs `IgnoredAny` | 1,262 | 1,230 | 858 | 2,956 |
| serde_json `Value` | 315 | 133 (68) | 225 (81) | 461 (349) |
| sonic-rs `Value` | 964 | 772 (126) | 350 (172) | 1,103 (1,215) |
| simd-json tape | 778 | 437 (97) | 284 (137) | 510 (498) |

Against the validate-only parsers, `feed` leads on bulk (with the split caveat), is level with serde_json on numbers, and trails on records and on one huge string. A profile puts that gap in per-token work rather than the byte scan. Its lasting advantage is memory: it never holds the document, so a body of any size streams through in 64 KiB.

Rewriting, tokenize and write back with the output drained every 64 KiB, against serde_json streaming the same tokens through `serde_transcode` (no tree, same drained output) and against parsing to a `Value` and writing it:

| MB/s (peak heap MB) | bulk | records | numbers | big_text |
|---|---|---|---|---|
| chunkjson `feed` into `Writer` | 743 (0.13) | 397 (0.13) | 328 (0.13) | 1,245 (0.13) |
| serde_json `serde_transcode` | 382 (0.13) | 314 (0.13) | 225 (0.13) | 462 (215) |
| serde_json `Value`, then write | 263 | 120 (79) | 136 (96) | 329 (349) |

`serde_transcode` is the fair streaming comparison: chunkjson leads by 1.3x on records to 2.7x on big_text. On big_text serde_json still copies the whole string, hence its 215 MB.
