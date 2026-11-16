# Streaming JSON tokenizer: push API and fast path: verified

- AC-1: `push_api_rebuilds_what_serde_json_parses`
- AC-2: `push_api_accepts_exactly_what_serde_json_accepts`, `push_and_pull_agree_on_any_bytes`, `push_and_pull_agree_on_json_with_multibyte_text` (a planted null-read-as-false in the fast loop failed the first two; a planted wrong string span failed the last)
- AC-2a: `push_and_pull_agree_at_tight_limits`, `push_and_pull_agree_on_any_bytes`, `limits_hold_through_the_fast_loop` (with either fast-loop limit guard removed, these fail)
- AC-3: `invalid_utf8_in_a_chunk_is_reported_where_pull_reports_it`
- AC-4: `a_sink_error_stops_the_feed`, `a_sink_error_inside_the_fast_loop_leaves_the_offset_after_the_refused_event` (fails without the offset update)
- AC-5: `cargo run --release` in `crates/chunkjson/bench`, 2026-11-16, shared 4-core host, provisional: feed against next in MB/s: bulk 1,289 vs 734, records 724 vs 345, numbers 415 vs 271, big_text 1,735 vs 1,334
