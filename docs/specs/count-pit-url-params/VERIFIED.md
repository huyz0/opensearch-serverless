# Verified: wildcard options, and `_search` URL parameters over a view

- AC-1: `wildcard_options_are_accepted_when_they_change_nothing` in crates/node/tests/it/search_url_params.rs (seen red as an unrecognized parameter, green after)
- AC-2: `wildcard_options_are_accepted_when_they_change_nothing` in crates/node/tests/it/search_url_params.rs
- AC-3: `the_url_parameters_of_a_search_apply_to_a_view_too` in crates/node/tests/it/frozen_search.rs (characterisation: it passed before this change, because the view is searched from the same parsed source)
