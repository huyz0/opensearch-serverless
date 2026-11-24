# Template REST routes: verified

Run on 2026-11-24 in a Linux sandbox: `cargo test -p oss-node --all-features --test server template_route assemble`, `cargo clippy -p oss-node --all-targets --all-features -- -D warnings`.

- AC-1: `a_template_is_stored_read_listed_and_removed`, `the_two_sets_are_separate`
- AC-2: `what_cannot_be_honoured_is_refused_and_stores_nothing`
- AC-3: `create_true_refuses_a_name_that_exists`
- AC-4: `a_read_of_an_unchanged_set_is_one_get_and_nothing_is_listed`
- AC-5: `the_template_routes_are_served_only_when_templates_are_on`
