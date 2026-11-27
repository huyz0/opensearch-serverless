# Verified: `ignore_unavailable` on `_field_caps` and `_msearch`

- AC-1: `ignore_unavailable_on_field_caps_skips_the_names_that_are_no_index` in crates/node/tests/it/search_several.rs (seen red as an unrecognized parameter, green after)
- AC-2: `ignore_unavailable_on_field_caps_skips_the_names_that_are_no_index` in crates/node/tests/it/search_several.rs
- AC-3: `ignore_unavailable_in_an_msearch_header_skips_the_names_that_are_no_index` in crates/node/tests/it/search_several.rs (seen red as a refused header key, green after)
- AC-4: `ignore_unavailable_in_an_msearch_header_skips_the_names_that_are_no_index` in crates/node/tests/it/search_several.rs
