# Reader: term-level and compound queries: verified

Run with `cargo test -p oss-reader -p oss-compat` and `scripts/gate.sh` on 2026-11-17 (Linux cloud container); the new fixture steps recorded from stock OpenSearch 3.10.0-SNAPSHOT build `b9f09249`.

- AC-1: `the_reader_node_answers_every_search_fixture_as_opensearch_did` (84 searches; seen red with a fuzzy score and an error column changed in the recording)
- AC-2: `terms_refuses_what_opensearch_refuses`, `string_queries_refuse_bad_shapes_and_unserved_options`, `fuzzy_refuses_what_opensearch_refuses`, `exists_reads_its_field_and_refuses_the_rest`, `ids_reads_values_and_wraps_its_failures_as_parsing`, `compound_queries_refuse_what_opensearch_refuses`
- AC-3: `terms_follows_the_field_type`, `terms_that_cannot_match_match_nothing`, `string_queries_on_other_types_are_shard_exceptions`, `fuzzy_builds_its_edits_from_the_term`, `exists_finds_fields_by_name_pattern_or_object`, `compound_queries_wrap_their_inner_queries`
- AC-4: `numeric_ids_pack_two_digits_a_byte`, `base64_ids_decode_and_escape_a_marker_byte`, `every_kind_reads_back_as_written`
