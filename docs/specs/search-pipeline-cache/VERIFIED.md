# Compiled search pipeline cache: verified

Run on 2026-11-25 in a Linux cloud sandbox (emulator store, so numbers are provisional): `cargo test -p oss-node --all-features --test server -- search_pipeline_route`.

- AC-1: `an_unchanged_pipeline_compiles_once_and_a_replaced_one_again`
- AC-2: `an_unchanged_pipeline_compiles_once_and_a_replaced_one_again`
- AC-3: `a_large_pipeline_is_not_kept_and_the_cache_is_bounded`
- AC-4: `a_search_resolves_its_pipeline_with_one_marker_get_and_no_list`
