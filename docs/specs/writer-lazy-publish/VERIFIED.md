# Lazy publish: verification

- AC-1: `a_shard_never_published_is_due_once_it_has_a_write_and_not_before` (writer, `crates/writer/src/publish/lazy.rs`). Seen failing against a stub answering "not due".
- AC-2: `a_shard_is_due_at_its_age_counted_from_its_oldest_unpublished_write` (writer, `crates/writer/src/publish/lazy.rs`).
- AC-3: `the_due_time_rounds_up_to_the_tick` (writer, `crates/writer/src/publish/lazy.rs`).
- AC-4: `operations_make_a_shard_due_before_its_age_and_zero_never_does` (writer, `crates/writer/src/publish/lazy.rs`).
- AC-5: `an_indexs_refresh_interval_replaces_the_nodes_age_and_minus_one_is_never_by_age` (writer, `crates/writer/src/publish/lazy.rs`).
- AC-6: `after_a_publish_a_shard_waits_for_a_later_write_and_ages_from_it` and `a_write_with_no_mark_is_aged_from_the_first_ask_that_sees_it` (writer, `crates/writer/src/publish/lazy.rs`).
- AC-7: `age_zero_is_eager` and `age_zero_is_eager_whatever_the_indexs_interval` (writer, `crates/writer/src/publish/lazy.rs`).
- AC-8: `a_forgotten_shard_is_due_once_again` (writer, `crates/writer/src/publish/lazy.rs`).
- AC-9: `the_settings_form_a_valid_registry_with_the_javas_defaults` (writer, `crates/writer/src/publish/lazy.rs`).
- AC-10: `lazy_publishing_makes_a_fifth_of_the_publishes_at_the_requests_of_one_blob_and_one_swap` (writer, `tests/lazy_publish.rs`): 20 shards, 10 s, 160 publishes against 800 eager, 2 PUT and 0 LIST each. Emulator, provisional.
