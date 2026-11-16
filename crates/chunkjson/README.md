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
- `Config { framing, max_depth, max_number_len }`; `Framing::{Single, Concatenated, Ndjson}`.
- `JsonReader::new(reader, buffer, config)` with `next_event()` over any `std::io::Read`, through a `Buffer` (implement it for a pooled buffer).
- `Writer::new(Config)`, then `event(Event, &mut out)` for tokenizer events, or the building calls `begin_object`, `end_object`, `begin_array`, `end_array`, `key`, `string`, `number` (JSON number text), `u64`, `i64`, `bool`, `null`; `check_complete()` says whether the output is complete. `out` is any `Output`: `Vec<u8>`, or `IoOutput` over a `std::io::Write`. A refused call returns `WriteError::Misuse` and writes nothing; after an output error every call is refused.

## Behaviour

- **Grammar:** RFC 8259, the same language serde_json accepts, checked by property tests and a fuzz target against it. Differences: a number outside f64's range is passed through as text (serde_json refuses it); nesting defaults to 128 (serde_json stops at 127). No comments, no trailing commas.
- **Memory:** the nesting stack and the number buffer are allocated once, by `Config`. A 64 MiB string fed in 64 KiB chunks grows the heap by nothing (`tests/memory.rs`).
- **NDJSON:** documents end at `\n`; `\r\n` and blank lines are allowed; a line break inside a document is `NewlineInDocument` and a second document on a line is `TrailingData`. Framing happens in the same scan as tokenizing.
- **Writing:** compact output; strings escaped as Jackson's default generator does (`"`, `\`, `\b \f \n \r \t`, other control characters as `\u00XX` in upper case; `/` and non-ASCII as is). In NDJSON and concatenated framing a newline follows each top-level value. The writer never writes invalid JSON: it refuses the call instead. Its only allocation is the nesting stack, so a 128 MiB string written in fragments grows the heap by nothing (`tests/writer_memory.rs`). Not yet: pretty printing, float formatting.

## Speed (provisional)

`bench/` (outside the workspace) runs each contender in its own process; MB/s is input bytes over the median run, peak heap is growth during the parse. Measured 2026-11-16 on a shared 4-core cloud Xeon (AVX-512), so relative only, and noisy by 10 to 20% between runs. `ignored` contenders validate without building anything (serde's `IgnoredAny`), and serde_json's does not check strings' UTF-8; chunkjson always does. `value`/`tape` build a document in memory.

| MB/s (peak heap MB) | bulk 21 MB NDJSON | records 10 MB | numbers 15 MB | big_text 135 MB, one doc |
|---|---|---|---|---|
| chunkjson `feed`, 64 KiB chunks ² | 1,440 (0) | 910 (0) | 615 (0) | 1,830 (0) |
| chunkjson `next`, 64 KiB chunks | 734 (0) | 345 (0) | 271 (0) | 1,334 (0) |
| oss-rest tokenizer, 64 KiB ¹ | 366 (0.05) | 330 (0.05) | 259 (0.05) | 405 (0.05) |
| serde_json `IgnoredAny` | 918 | 1,142 | 625 | 2,137 |
| serde_json `Value` | 305 | 93 (68) | 194 (81) | 400 (349) |
| sonic-rs `IgnoredAny` | 891 | 877 | 761 | 2,307 |
| sonic-rs `Value` | 745 | 445 (126) | 307 (172) | 831 (1,215) |
| simd-json tape | 641 | 242 (97) | 218 (137) | 444 (498) |

¹ Measured 2026-11-15 with a contender since removed, because this crate's bench may not depend on the workspace. That comparison belongs in `oss-rest`.

² Re-measured 2026-11-16 after the fast loop's number scan and string inlining, midpoint of two runs; serde_json `IgnoredAny` measured alongside: 1,016, 1,125, 669, 2,199.

Writing, tokenize and write back in 64 KiB chunks against serde_json parsing to a `Value` and writing it, MB/s (peak heap MB), 2026-11-16: bulk 618-664 (0.13) against 257; records 349-351 (0.13) against 79-81 (79); numbers 212 (0.13) against 122-126 (96); big_text 1,182-1,242 (0.13) against 302-307 (349).

`feed` beats every parser on NDJSON bulk bodies and every document-building parser everywhere, with no heap growth, and is level with serde_json's validate-only mode on numbers. It still trails the validate-only parsers on key-dense records and on one huge string. A profile puts that gap in per-token work rather than the byte scan, so SIMD classification alone would not close it.
