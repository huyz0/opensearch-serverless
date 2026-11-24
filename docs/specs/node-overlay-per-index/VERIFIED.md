# Verified: the tail overlay, switched per index

Run with `cargo test -p oss-node --features writer --test overlay_search` on the emulated file store, in a Linux sandbox; numbers are provisional.

- AC-1: `the_index_setting_is_an_index_scope_switch_on_by_default` in crates/node/tests/overlay_search.rs
- AC-2: `an_index_that_turns_the_overlay_off_is_answered_from_the_commit` in crates/node/tests/overlay_search.rs
- AC-2: `an_index_that_turns_it_on_or_says_nothing_gets_the_overlay` in crates/node/tests/overlay_search.rs
- AC-3: `an_unparseable_index_value_counts_as_on` in crates/node/tests/overlay_search.rs
