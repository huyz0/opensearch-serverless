# Listing index names: verified

Run with `cargo test -p oss-meta` and the whole gate, `scripts/gate.sh`, on 2026-11-15 (Linux sandbox). Each listed defect was planted and seen to fail: a page that does not resume from the last name read (the walk never ends: the test hangs), a page that takes one name past its limit (`every_index_is_walked_once_in_name_order` and two more), and a prefix that lets `[` through (`a_prefix_is_refused_with_the_javas_message`).

- AC-1: `every_index_is_walked_once_in_name_order`, `a_page_honours_its_prefix_and_cursor`, `a_key_below_a_name_is_not_a_name`
- AC-2: `a_page_costs_one_list_and_a_read_a_name`, `tombstones_cost_reads_not_lists`, `a_page_short_of_a_full_list_lists_on_from_the_last_key`, `a_thousand_names_are_one_list`
- AC-3: `names_within_the_cap_are_listed_unread`, `past_the_cap_only_live_names_count`, `more_live_names_than_the_cap_are_refused`, `a_thousand_names_are_one_list`
- AC-4: `a_prefix_is_refused_with_the_javas_message`, `a_valid_prefix_passes`, `a_bad_prefix_or_bound_sends_nothing`
- AC-5: `a_malformed_register_fails_the_page`, `a_tombstone_is_recognised_not_parsed`, `a_register_longer_than_the_buffer_fails_the_page`
