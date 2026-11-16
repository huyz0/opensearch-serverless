# Streaming JSON writer: verified

- AC-1: `rewriting_tokenizer_events_gives_serde_json_bytes`, `rewriting_ndjson_keeps_one_value_per_line` (both failed against a stub writer before the writer existed)
- AC-2: `any_call_sequence_writes_only_valid_json` (all three framings), `errors_say_what_was_refused`, `misuse_is_refused_with_its_kind_and_writes_nothing`, `an_output_error_is_returned_and_the_writer_then_refuses`
- AC-3: `a_string_in_fragments_writes_the_same_bytes`
- AC-4: `strings_are_escaped_as_jackson_escapes_them`, `every_stopping_byte_has_an_escape`
- AC-5: `a_128_mib_string_is_written_in_64_kib_fragments_without_heap_growth` (counts every allocation, not the live heap: a planted allocate-and-free in the escaper fails it)
- AC-6: `cargo run --release` in `crates/chunkjson/bench`, 2026-11-16, shared 4-core host, provisional, two runs each. Tokenize and write back (chunkjson/rewrite-64k) against serde_json parse to a Value and write it back (serde_json/value-rewrite), in MB/s with peak heap: big_text 1,182-1,242 (0.13 MB) against 302-307 (349 MB); records 349-351 (0.13 MB) against 79-81 (79 MB); numbers 212-213 against 122-126; bulk 618-664 against 257-258.
