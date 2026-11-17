# Reader: match_phrase and multi_match: verified

Run with `cargo test -p oss-reader -p oss-compat` and `scripts/gate.sh` on 2026-11-17 (Linux cloud container); the new fixture steps recorded from stock OpenSearch 3.10.0-SNAPSHOT build `b9f09249`.

- AC-1: `the_reader_node_answers_every_search_fixture_as_opensearch_did` (seen red with the sloppy phrase's recorded score changed)
- AC-2: `match_phrase_reads_short_and_long_forms`, `match_phrase_refuses_what_opensearch_refuses`, `multi_match_reads_its_parameters`, `multi_match_refuses_what_opensearch_refuses`
- AC-3: `match_phrase_builds_a_phrase_on_text_and_a_term_elsewhere`, `multi_match_combines_each_field_s_query`, `multi_match_is_lenient_when_asked_or_over_every_field`
