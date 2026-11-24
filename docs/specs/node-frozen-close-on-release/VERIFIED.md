# Verified: node-frozen-close-on-release

- AC-1: `releasing_a_prefix_takes_every_index_under_it_and_no_other` in crates/node/src/open_indexes.rs
- AC-2: `closing_a_view_frees_its_shards_and_the_next_search_opens_them_afresh` in crates/node/tests/it/frozen_search.rs
- AC-3: `closing_a_view_frees_its_shards_and_the_next_search_opens_them_afresh` in crates/node/tests/it/frozen_search.rs
- AC-4: `a_release_tells_the_hook_each_view_it_removed_and_no_other` in crates/node/tests/pit_route.rs

NOT-RUN: coverage (no cargo-llvm-cov in the sandbox), the assembled-node path where the hook is installed (assemble.rs), tests observed red before the code.
