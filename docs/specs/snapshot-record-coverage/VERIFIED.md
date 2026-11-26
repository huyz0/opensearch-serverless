# Verified: snapshot-record-coverage

- AC-1: `a_finished_index_names_every_shard_exactly_once`, `a_shard_without_its_number_is_refused`.
- AC-2: `an_index_with_no_shards_listed_is_still_the_marker`.
- AC-3: `the_shard_count_is_bounded_before_anything_is_allocated`.
- AC-4: `a_finished_index_must_name_every_shard_once`, `the_shard_list_is_bounded_before_it_is_allocated`.
- AC-5: `snapshot_java_fixtures`.

The AC-1 and AC-3 summary tests were seen red (three of four failed) before the code. The restore-reader tests in AC-4 were written with the code and not seen red. AC-2 passed from the start because it pins existing behaviour.
