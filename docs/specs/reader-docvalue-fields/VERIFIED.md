# Reader: docvalue_fields: verified

Run with `cargo test -p oss-reader -p oss-compat` and `scripts/gate.sh` on 2026-11-17 (Linux cloud container); the new fixture steps recorded from stock OpenSearch 3.10.0-SNAPSHOT build `b9f09249`.

- AC-1: `the_reader_node_answers_every_search_fixture_as_opensearch_did`
- AC-2: `entries_are_names_or_objects`, `a_bad_entry_fails_as_opensearch_fails`, `at_most_a_hundred_entries`
- AC-3: `names_and_patterns_resolve_in_hash_map_order`, `a_field_without_doc_values_fails_as_opensearch_fails`
- AC-4: `a_hit_s_fields_are_written_from_its_doc_values`
