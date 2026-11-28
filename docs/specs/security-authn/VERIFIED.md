# Security authentication: evidence

- AC-1: `a_refused_request_gets_the_guards_answer_and_a_challenge`, `a_path_no_route_serves_is_refused_before_it_is_found`, `an_admitted_request_reaches_its_handler_knowing_who_it_is` and `a_refused_request_with_a_body_closes_the_connection` in crates/rest/tests/guard.rs
- AC-2: `an_assembled_node_refuses_every_request_without_valid_credentials_alike` in crates/node/tests/server/security_authn.rs; `an_assembled_node_authenticates_and_authorizes_as_the_stock_plugin_did` in crates/compat/tests/node_cluster.rs (the recorded `no credentials`, `wrong password` and `unknown user` steps); `a_basic_header_is_a_user_and_a_password_and_nothing_else_is` in crates/coordinator/tests/it/security_authn.rs
- AC-3: `a_user_created_through_the_api_authenticates_at_once_on_that_node` in crates/node/tests/server/security_authn.rs
- AC-4: `an_assembled_node_authenticates_and_authorizes_as_the_stock_plugin_did` (the recorded whoami step); `a_user_holds_the_roles_its_mappings_name` and `authinfo_is_the_recorded_shape` in crates/coordinator/tests/it/security_authn.rs
- AC-5: `a_node_that_cannot_read_the_configuration_answers_503` and `a_snapshot_older_than_three_intervals_is_not_used` in crates/node/tests/server/security_authn.rs
- AC-6: `a_full_verification_pool_answers_429`, `five_wrong_passwords_throttle_that_user_only` and `a_failed_check_is_never_cached` in crates/node/tests/server/security_authn.rs
- AC-7: `a_deleted_user_stops_authenticating_on_that_node_at_once`, `a_change_on_another_node_is_seen_after_the_interval` and `the_credential_cache_is_bounded` in crates/node/tests/server/security_authn.rs
- AC-8: `the_snapshot_refreshes_lazily_and_idle_nodes_make_no_requests` in crates/node/tests/server/security_authn.rs
- AC-9: `security_on_without_an_administrator_does_not_start` and `with_security_off_no_guard_is_installed` in crates/node/src/admin_assemble.rs; `the_administrator_is_a_pair_and_the_refresh_is_bounded` in crates/node/src/boot/config/tests.rs
- AC-10: `a_clock_that_goes_back_does_not_keep_a_copy_fresh`, `a_change_made_here_does_not_read_the_stores_on_every_request_while_they_are_down` and `a_change_made_here_is_seen_by_the_next_request_even_inside_the_interval` in crates/node/tests/server/security_hardening.rs
- AC-11: `an_unknown_name_is_throttled_like_a_known_one` in crates/node/tests/server/security_hardening.rs; `security_on_without_an_administrator_does_not_start` in crates/node/src/admin_assemble.rs
- AC-12: `configuring_an_administrator_turns_security_on_and_the_variable_still_wins` in crates/node/src/boot/config/tests/security.rs
