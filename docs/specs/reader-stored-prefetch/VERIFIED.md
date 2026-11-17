# Reader: stored-fields prefetch: verified

Run with `cargo test -p oss-reader -p oss-compat` on 2026-11-17 (Linux cloud container, emulator store, provisional numbers).

- AC-1: `the_hits_of_a_cold_shard_are_fetched_in_one_round`. Seen red first: without the announcement the fetch made 3 reads in 3 round trips; with it, 1 read in 1.
- AC-2: `the_reader_node_answers_every_search_fixture_as_opensearch_did` and the reader's tests.
