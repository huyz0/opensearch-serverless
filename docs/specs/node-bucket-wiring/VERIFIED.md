# Verified

- AC-1: `the_buckets_switch_is_off_by_default_and_reads_a_flag`
- AC-2: `a_bucketed_index_has_a_member_register_and_others_none`
- AC-3: `a_member_shards_publish_lands_in_its_bucket_entry`
- AC-4: `a_shard_of_an_unbucketed_index_publishes_to_its_own_register`
- AC-5: `a_node_with_the_switch_off_has_no_bucket_wiring`
- AC-6: `a_registered_node_reports_the_members_it_holds_open`
- AC-7: `a_one_shard_create_joins_a_bucket_and_a_sharded_one_does_not`, `a_create_without_the_join_names_no_bucket`
- AC-7 (the join itself): `a_join_places_the_index_in_a_bucket_listing_it_as_a_member`
- AC-8: `a_reader_opens_a_member_from_its_bucket_entry`, `a_member_with_no_commit_yet_is_not_followable`
- AC-8 (reader crate): `a_member_opens_to_the_files_its_entry_names_with_two_gets`, `a_bucketed_index_has_only_shard_zero`, `a_member_without_a_commit_or_a_manifest_entry_is_not_followable`
- AC-7 (leaving): `a_create_that_fails_after_joining_leaves_the_bucket`, `leaving_takes_the_index_back_out_of_its_bucket`
- AC-9: `a_create_names_its_group_by_the_bucket_group_setting`, `an_index_that_sets_the_bucket_policy_off_joins_none`, `a_bad_bucket_policy_is_refused_and_other_settings_still_are`
