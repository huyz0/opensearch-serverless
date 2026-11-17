# Zones: evidence

- AC-1: `the_zone_is_node_attr_zone_and_blank_is_none`, `a_renamed_awareness_attribute_is_read_by_its_name`, `the_array_form_is_read_when_the_key_is_not_set`, `the_array_form_is_read_up_to_its_bound_and_not_past` (cluster). Planted: no array-form read fails the third.
- AC-2: `a_zone_survives_renewal_and_reaches_peers_through_the_digest` (cluster, emulator).
- AC-3: `an_exchange_is_classified_as_the_javas`, `bytes_are_totalled_and_broken_down_by_action`, `concurrent_counts_are_exact`, `past_the_action_bound_bytes_go_to_other_and_the_total_stays_exact` (cluster). Planted: counting zero bytes fails the second.
- AC-4: `the_zone_preference_keeps_search_bytes_inside_the_zone`, `a_fleet_without_zones_reads_as_unmeasured` (cluster). Modelled, not measured across a network: cross-zone search bytes 2,064,000 -> 0 (480 of 720 shard searches crossed zones with the preference off; none with it on, the zone mate or the coordinator itself serving), 1,548,000 same-zone. Planted: a preference that never moves a local reader first leaves 2,064,000 -> 2,064,000 and fails.
