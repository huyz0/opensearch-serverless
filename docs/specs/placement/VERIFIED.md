# Placement: evidence

- AC-1: `murmur3_and_hash_code_are_the_javas`, `decimal_is_javas_integer_to_string` (cluster).
- AC-2: `every_writer_owner_is_the_javas`, `equal_weights_go_to_the_lower_id_in_javas_order` (cluster). Planted: a tie broken the other way fails the second; a takeover start from murmur3 fails `every_fleet_decision_is_the_javas`.
- AC-3: `every_reader_rank_is_the_javas` (cluster).
- AC-4: `every_zone_preference_is_the_javas`, `zones_compare_as_the_javas`, `a_zone_local_reader_is_chosen_until_it_runs_more_than_the_spill_beyond` (cluster).
- AC-5: `every_fleet_decision_is_the_javas`, `nobody_live_has_no_writer_and_no_winner`, `a_departed_zone_is_read_once_and_a_failed_read_again`, `an_assignment_needs_a_positive_term`, `every_decision_at_its_thresholds_is_the_javas` (400 random small fleets the Java recorded, loads drawn around each threshold), `a_writer_at_its_cap_has_no_room_for_writes_or_takeovers`, `departed_zones_are_forgotten_all_at_once_at_the_bound` (cluster).
- AC-6: `a_bad_config_is_refused_and_the_default_is_the_javas`, `a_cold_shard_keeps_placements_order`, `the_rate_sets_the_width_and_decays`, `a_hot_shard_spreads_over_its_width_and_avoids_the_busier_pick`, `past_the_tracking_bound_cold_shards_are_dropped`, `a_steady_rate_reads_the_slow_average_once_a_burst_fades`, `below_the_tracking_bound_nothing_is_dropped_and_a_warm_shard_never_is` (cluster).

Mutants over the diff (`cargo mutants --in-diff`, oss-cluster): 305 tested, none missed after the tests above, 2 timeouts (a `decimal` that never ends), 6 equivalent mutants excluded in `.cargo/mutants.toml` with the reason.
