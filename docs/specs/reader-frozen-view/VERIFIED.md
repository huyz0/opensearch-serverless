# Reader frozen view: verified

Run on 2026-11-19 in a Linux sandbox, emulator numbers provisional: `cargo test -p oss-reader --test it frozen`.

- AC-1: `a_frozen_commit_resolves_like_the_live_one_with_one_get`.
- AC-2: `the_engine_opens_a_frozen_commit_without_the_commit_register`.
- AC-3: `a_view_of_another_incarnation_is_not_held_and_one_without_a_uuid_is`.
- AC-4: `a_view_still_capturing_or_without_the_shard_is_not_frozen`.
- AC-5: `a_file_without_a_length_or_a_followable_location_is_refused`.
- AC-6: `a_missing_index_is_not_found_after_one_get`.
- Shared cache (Risks): `a_frozen_open_costs_what_a_second_live_open_does_on_a_shared_cache`.
- AC-7: `a_bucketed_views_node_packed_files_resolve_with_one_get` and `a_bucketed_views_files_in_its_shards_own_path_resolve_under_shard_zero` (`crates/reader/tests/it/frozen_bucket.rs`).
