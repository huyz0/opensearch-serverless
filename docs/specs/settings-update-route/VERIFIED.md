# Verified: settings-update-route

- AC-1: `an_update_stores_the_value_and_bumps_the_version` in crates/node/tests/server/settings_route.rs
- AC-2: `a_refused_update_changes_nothing` in crates/node/tests/server/settings_route.rs
- AC-3: `an_update_that_changes_nothing_writes_nothing` in crates/node/tests/server/settings_route.rs
- AC-4: `a_missing_index_is_404` in crates/node/tests/server/settings_route.rs
- AC-5: `a_null_removes_and_preserve_existing_keeps` in crates/node/tests/server/settings_route.rs
- AC-6: `every_registered_route_has_an_action_row` in crates/node/src/security_authz.rs
