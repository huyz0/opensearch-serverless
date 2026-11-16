# Streaming JSON tokenizer: cheaper tokens in the fast loop: verified

- AC-1: `push_and_pull_agree_on_any_bytes`, `push_and_pull_agree_at_tight_limits`, `push_and_pull_agree_on_json_with_multibyte_text`, `push_api_accepts_exactly_what_serde_json_accepts`, `push_api_rebuilds_what_serde_json_parses`, unchanged and passing
- AC-2: `a_number_cut_by_the_chunk_end_is_left_to_the_general_machine`, `digit_lanes_flag_every_non_digit_byte`, `digit_runs_end_at_the_first_non_digit_or_are_cut`
- AC-3: `cargo run --release` in `crates/chunkjson/bench`, 2026-11-16, shared 4-core host, provisional, two runs each: feed in MB/s before and after: bulk 1,289 and 1,388-1,495, records 724 and 864-958, numbers 415 and 610-617, big_text 1,735 and 1,761-1,894. Instructions per byte under callgrind, before and after: records 20.6 and 16.4, numbers 28.5 and 18.6, bulk 10.8 and about 10.4.
