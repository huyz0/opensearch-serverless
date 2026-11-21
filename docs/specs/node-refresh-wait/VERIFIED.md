# Verified: node-refresh-wait

- AC-1: `a_confirmed_wait_makes_the_refresh_publish_nothing` in crates/node/tests/refresh_wait.rs
- AC-2: `everywhere_asks_the_wait_to_cover_grants` in crates/node/tests/refresh_wait.rs
- AC-3: `a_wait_that_does_not_confirm_falls_back_to_publishing` in crates/node/tests/refresh_wait.rs
- AC-4: `the_owner_visibility_never_waits_and_publishes` in crates/node/tests/refresh_wait.rs
- AC-5: `with_no_sink_a_waiting_visibility_publishes` in crates/node/tests/refresh_wait.rs
