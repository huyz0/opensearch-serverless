# The node's create applies index templates: verified

Run on 2026-11-24 in a Linux sandbox: `cargo test -p oss-node --all-features --test server index_route`, `cargo clippy -p oss-node --all-targets --all-features -- -D warnings`.

- AC-1: `a_create_inherits_the_mapping_and_shards_of_the_matching_template`
- AC-2: `a_template_that_carries_settings_is_refused_not_dropped`
- AC-3: `a_create_with_no_template_stored_reads_only_the_marker`
- AC-4: `a_create_is_one_conditional_create_and_a_head_one_read`
