# Verified

- AC-1: `terms_range_ids_and_exists_read` and `bool_keeps_every_clause_and_minimum_should_match` in crates/transport/tests/search_shard.rs, against `fixtures/search-shard/java-3.10.0.txt` written by `fixtures/search-shard/WriteSearchShard.java` on the Java shell at e71d4233 (3.10.0-SNAPSHOT)
- AC-1b: `the_first_unsupported_item_is_named_and_nothing_after_it_is_read` in crates/transport/tests/search_shard.rs
- AC-2: `an_answer_with_hits_is_what_the_java_shell_writes_byte_for_byte` in crates/transport/tests/search_shard.rs
- AC-3: NOT-RUN: not built yet.
- AC-4: NOT-RUN: not built yet.
- AC-4b: NOT-RUN: not built yet.
- AC-5: NOT-RUN: the request limits are held by `a_request_over_a_limit_is_refused_and_one_at_it_is_not` and the answer limit by `an_answer_past_the_limit_is_refused_whole` in crates/transport/tests/search_shard.rs; the MAC order, the length-versus-bytes check and the negotiated version are not built yet
- AC-6: NOT-RUN: not built yet.
- AC-7: NOT-RUN: not built yet.
- AC-8: NOT-RUN: not built yet.
- AC-9: NOT-RUN: not built yet.
- AC-10: NOT-RUN: not built yet.
- AC-11: NOT-RUN: not built yet.
