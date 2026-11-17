# Reader: simple_query_string: verified

Run with `cargo test -p oss-reader -p oss-compat` on 2026-11-17 (Linux cloud container); the new fixture steps recorded from stock OpenSearch 3.10.0-SNAPSHOT build `b9f09249`, and the expected queries of the grammar tests compared with `_validate/query?explain=true` on the same build.

- AC-1: `the_reader_node_answers_every_search_fixture_as_opensearch_did`
- AC-2: `the_defaults_are_those_of_opensearch`, `every_key_is_read`, `flags_are_names_or_numbers`, `a_field_is_a_name_with_an_optional_boost`, `parse_errors_are_opensearchs`
- AC-3: `words_are_or_ed_or_and_ed_by_the_default_operator`, `plus_pipe_and_minus_set_each_clause`, `groups_nest_and_unbalanced_ones_are_read_leniently`, `phrases_take_their_slop`, `prefix_is_lower_cased_by_the_field_analyzer`, `fuzziness_defaults_to_two_and_is_capped`, `flags_switch_each_feature_off`, `flags_are_names_or_a_number`, `escapes_make_a_character_part_of_a_word`, `empty_and_star_queries`
- AC-4: `several_fields_are_dis_maxed_with_a_tie_of_one`, `a_prefix_over_fields_is_a_dis_max_of_each`, `minimum_should_match_applies_to_a_top_level_boolean_only`, `a_number_in_text_fails_unless_lenient`, `too_many_clauses_fail`, `a_group_nested_past_the_limit_is_refused`
- AC-5: `what_is_not_served_is_refused`
