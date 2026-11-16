# Streaming JSON writer

## Serves

[ADR 0004](../../adr/0004-bounded-memory-and-streaming.md) and [streaming-json](../streaming-json/SPEC.md): responses and the write log must write JSON, including one huge string (extracted PDF or Confluence text), without building the document or the string in memory.

## Change

A sans-IO writer in `chunkjson`, the mirror of the tokenizer:

- `Writer::new(Config)` and `Writer::event(Event, &mut impl Output)`. A writer takes the same events the tokenizer yields, so `tokenizer -> writer` re-serialises any input. Strings and keys are written fragment by fragment (`first`/`last`), so a string of any size passes through in bounded memory.
- Convenience methods for building output: `begin_object`, `end_object`, `begin_array`, `end_array`, `key`, `string`, `number` (JSON number text, grammar-checked), `u64`, `i64`, `bool`, `null`.
- `Output` is a small write trait. It is implemented for `Vec<u8>` and, through `IoOutput`, for any `std::io::Write`. An async caller writes into a `Vec`, then drains it between calls. Fragments keep that buffer bounded.
- Compact output. Strings are escaped as Jackson's default generator escapes them, which is what OpenSearch writes: `"`, `\`, `\b \f \n \r \t`, and `\u00XX` with upper-case hex for the other control characters below U+0020. `/` and non-ASCII are written as is.
- Framing follows `Config`. In NDJSON and concatenated framing a newline follows each top-level value. `EndDocument`, `NeedMore` and `Eof` events write nothing, so tokenizer events can be piped straight in.
- The writer refuses misuse and never writes invalid JSON: a key outside an object, a value where a key is due, a mismatched or extra end, a fragment that does not continue the open string, nesting beyond `max_depth`, a second top-level value in single framing, and `check_complete` with a value still open. Each refusal is a typed `WriteError`. Nothing is written for the refused call, and the writer stays usable.
- Runs of plain string bytes are found with the tokenizer's eight-bytes-at-a-time scan and copied in one write each.
- No `unsafe`, no new dependency, no allocation after `Writer::new`.

Not in this change: pretty printing, formatting floating-point numbers (the caller passes number text), an async adapter, and wiring into `oss-rest` or the write log.

## Acceptance criteria

- AC-1: for any JSON value, tokenizing its compact serde_json text in chunks of any size and writing the events back produces the same bytes as serde_json, except that `\u00XX` escapes use upper-case hex (property test).
- AC-2: for any sequence of writer calls, including misuse, the output written so far is always a prefix of valid JSON. After `check_complete` succeeds, serde_json parses it to the value the calls describe. A refused call writes nothing and changes no state (property test against a reference model).
- AC-3: a string split into fragments at any character boundaries gives the same bytes as the string written whole (property test).
- AC-4: strings are escaped as Jackson does: every byte below U+0020, `"`, `\`, `/`, `DEL` and non-ASCII text, checked byte for byte (unit test).
- AC-5: writing a 128 MiB string in 64 KiB fragments allocates nothing after `Writer::new` (counting allocator test, in its own test binary).
- AC-6: on the bench host, re-serialising input through the tokenizer and the writer is at least as fast as serde_json parsing it to a `Value` and writing that back, on `big_text` and `records` (`bench/`, provisional).

## Test plan

- AC-1, AC-2, AC-3: `tests/writer.rs`, property tests with serde_json as the oracle.
- AC-4: `tests/writer_calls.rs`.
- AC-5: `tests/writer_memory.rs`.
- AC-6: `cargo run --release` in `crates/chunkjson/bench`, new writer cases.

## S3 request budget

Unchanged: no S3 calls.

## Switch

None: a new API, nothing existing changes.

## Risks

- A writer bug writes JSON that readers refuse, or JSON that changes meaning. AC-1 and AC-2 compare against serde_json on random values and random call sequences, and the writer refuses rather than guesses.
