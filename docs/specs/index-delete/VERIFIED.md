# Verified

- AC-1: `a_read_finds_the_heads_of_the_uuid_one_get_a_shard`
- AC-2: `a_head_is_deleted_only_at_the_generation_read`
- AC-3: `delete_all_of_removes_only_the_uuids_heads_and_counts_them`
- AC-4: `a_release_clears_the_owner_and_keeps_the_term`
- AC-5: `a_capture_in_progress_pins_every_shard`, `a_view_of_another_index_pins_nothing`
- AC-6: `a_delete_removes_the_descriptor_heads_and_unowned_bytes`
- AC-7: `a_shard_a_point_in_time_names_keeps_its_bytes`
- AC-8: `an_owned_shard_waits_for_the_drain`
- AC-9: `a_missing_index_is_404_and_several_names_are_refused`
- AC-10: `scripts/tree-checks.sh` runs without the script; its self-test and gate rows are removed. Seen: `scripts/tree-checks.sh` green
