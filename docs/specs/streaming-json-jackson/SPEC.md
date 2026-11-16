# Streaming JSON tokenizer: what OpenSearch's Jackson parser also does

## Serves

Folding `oss-rest`'s own JSON tokenizer (`crates/rest/src/json/`) into `chunkjson`, so the repository has one tokenizer. `oss-rest` accepts what OpenSearch's Jackson factory accepts: JSON plus comments, an empty body, and errors and token locations in lines and columns (`JsonXContent` at huyz0/OpenSearch@49f238ed). This change gives `chunkjson` those abilities. A later change rebuilds `oss-rest`'s tokenizer on them and keeps its API.

## Change

- `Config::comments` (default `false`): `//` to the end of the line and `/* ... */` count as whitespace wherever whitespace may appear, and in both the pull and the push API. An unterminated block comment at `eof` is `UnexpectedEof`. A `/` followed by anything but `/` or `*` is `UnexpectedByte(b'/')` at the slash, as `oss-rest` reports it. In NDJSON framing a block comment may not span a line break, inside a document or between two, since NDJSON splits at every one: that is `NewlineInDocument`. With `comments` off, `/` is refused as before.
- `Config::allow_empty` (default `false`): in single framing, input of only whitespace (and comments) ends with `Eof` instead of `UnexpectedEof`.
- Positions: the tokenizer counts lines as it passes line breaks in whitespace and comments. It never scans past what it already reads.
  - `Position { offset, line, column }`, where line and column are 1-based and the column counts bytes, as `oss-rest`'s `Location` does.
  - Every event has a start: the opening quote of a key or string (for every fragment of it), the first byte of a number or literal, or the bracket. `EndDocument` starts at the document's start.
  - The push API passes it through a new `Sink::event_at(event, start)`, which by default calls `Sink::event`. The pull API returns it with the event from `Tokenizer::next_at`, since a pulled event borrows the tokenizer, and `Tokenizer::event_start()` gives it once the event is dropped.
  - `Error::line()` and `Error::column()` give the fault's position alongside its offset.
- `Tokenizer::reset()` returns the tokenizer to its state after `new`, keeping its allocations, so a caller that tokenizes one document per line does not allocate per line.
- No `unsafe`, no new dependency. The defaults keep today's behaviour exactly.

## Acceptance criteria

- AC-1: with `comments` and `allow_empty` off, events, errors and offsets are unchanged: the existing property and unit tests pass, changed only where they build a `Config` by hand, which gains `..Config::default()` for the new fields.
- AC-2: with `comments` on, inserting comments at any whitespace position of any JSON text, chunked at any sizes, gives the same events as the text without them, through `next` and `feed` (property test). The refusals named above are typed and at the right offset (unit tests).
- AC-3: for any sequence of JSON texts in concatenated framing, with random whitespace, line breaks and comments, chunked at any sizes, every event's start (offset, line, column) equals the position computed independently from the full text (`EndDocument`'s equals its first token's), through `next` and `feed`, and an error's line and column match its offset (property tests).
- AC-4: `allow_empty` makes whitespace-and-comment-only input end with `Eof` and no events. Without it, such input is `UnexpectedEof` (unit tests).
- AC-5: after `reset`, a tokenizer gives the same events as a new one, and `reset` allocates nothing (unit test with the counting allocator).
- AC-6: with the default config, `feed` costs at most 5% more instructions per byte than before, under callgrind on `records` and `bulk` (`bench/`).

## Test plan

- AC-1: the existing tests.
- AC-2 to AC-4: `tests/jackson.rs`.
- AC-5: `tests/memory.rs` or a new test binary.
- AC-6: callgrind on the bench binary, before and after.

## S3 request budget

Unchanged: no S3 calls.

## Switch

`comments` and `allow_empty` are settings of the tokenizer, off by default, because plain JSON readers must keep refusing comments. Positions cost a few instructions per event and are always on, if AC-6 holds.

## Risks

- Position tracking could slow the hot loop. AC-6 bounds it, and the fast loop keeps its token start from the index it already has.
- Comments add a second meaning to `/`. With `comments` off, `/` must be refused exactly as before; AC-1's tests cover that.
