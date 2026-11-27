# Verified: point-in-time routing and preference

- AC-1: `a_preference_is_taken_and_changes_nothing` in crates/node/tests/pit_route_limits.rs (seen red while a preference answered 501, green after)
- AC-2: `routing_and_preference_do_not_go_with_a_view` in crates/node/tests/it/frozen_search.rs (seen red while the parameters were ignored, green after)
- AC-3: `what_cannot_be_frozen_is_refused_with_the_javas_status` in crates/node/tests/pit_route.rs
