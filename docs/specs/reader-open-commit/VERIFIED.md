# Reader: open a published commit: verified

Run with `cargo test -p oss-reader` and `scripts/gate.sh` on 2026-11-15 (Windows desktop).

- AC-1: `the_commit_is_found_with_two_gets_and_no_list`
- AC-2: `the_engine_opens_both_segments_with_ranged_reads_only`
- AC-3: `the_engine_store_lists_sizes_and_versions_from_the_manifest_alone`
- AC-4: `a_whole_file_read_is_one_ranged_get_of_exactly_that_file`
- AC-5: `a_missing_index_is_not_found_after_one_get`
