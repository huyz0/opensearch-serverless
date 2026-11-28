# Security plugin, complete and native: evidence

## Acceptance evidence, slice 1

- AC-1: `a_list_holds_every_document_as_a_read_renders_it` and `an_empty_kind_lists_as_an_empty_object` in crates/node/tests/server/security_patch.rs; the same names in crates/coordinator/tests/it/security_patch.rs
- AC-2: `a_list_costs_a_marker_and_a_get_a_document` in crates/node/tests/server/security_patch.rs
- AC-3: `a_list_over_the_bound_is_refused_whole` in crates/node/tests/server/security_patch.rs
- AC-4: `patch_applies_each_operation`, `a_patch_that_makes_an_invalid_document_changes_nothing` and `a_patch_of_the_wrong_shape_is_400` in crates/node/tests/server/security_patch.rs; `the_recorded_messages_are_the_plugins` in crates/coordinator/tests/it/security_patch.rs; `a_patch_that_makes_an_invalid_document_is_refused_as_a_put_would` in crates/coordinator/tests/it/security_patch.rs
- AC-5: `a_patch_of_an_absent_document_is_404` and `a_patched_password_is_stored_as_a_hash` in crates/node/tests/server/security_patch.rs
- AC-6: `a_bulk_patch_changes_several_documents`, `an_invalid_operation_in_a_bulk_patch_writes_nothing` and `removing_what_is_not_there_changes_nothing` and `a_bulk_patch_over_100_documents_is_refused` in crates/node/tests/server/security_patch.rs
- AC-7: `a_patch_costs_what_a_replacing_put_costs` in crates/node/tests/server/security_patch.rs
- AC-8: `listing_a_kind_is_refused_to_a_user_and_served_to_an_administrator` in crates/node/tests/server/security_authn.rs; `every_registered_route_has_an_action_row` in crates/node/src/security_authz.rs
- AC-9: `with_security_off_no_guard_is_installed` in crates/node/src/admin_assemble.rs

## Slice 2

- AC-10: `the_built_in_set_is_the_recorded_one` in crates/coordinator/tests/it/security_builtin.rs; `a_list_holds_the_built_in_documents_with_their_flags` and `a_built_in_is_read_by_name_without_a_store_request` in crates/node/tests/server/security_builtin.rs
- AC-11: `a_change_to_a_shipped_document_is_refused_as_the_plugin_refuses_it` in crates/coordinator/tests/it/security_builtin.rs; `a_built_in_cannot_be_written_patched_or_deleted` and `a_bulk_patch_naming_a_built_in_writes_nothing` in crates/node/tests/server/security_builtin.rs
- AC-12: `a_mapping_for_a_built_in_role_is_accepted` in crates/node/tests/server/security_builtin.rs; `a_reserved_role_grants_what_the_plugin_grants` in crates/coordinator/tests/it/security_builtin.rs
- AC-13: `no_user_or_mapping_is_shipped` in crates/coordinator/tests/it/security_builtin.rs; `no_demo_user_is_shipped` in crates/node/tests/server/security_builtin.rs

## Slice 3

- AC-14: `securityconfig_answers_the_effective_configuration` in crates/node/tests/server/security_config_api.rs
- AC-15: `the_configuration_that_stock_keeps_closed_is_403_and_makes_no_request` in crates/node/tests/server/security_config_api.rs
- AC-16: `permissionsinfo_names_the_caller` in crates/node/tests/server/security_config_api.rs
- AC-17: `flushing_the_cache_makes_the_next_request_read_the_sets` and `a_user_without_admin_access_is_refused_every_one_of_these_routes` in crates/node/tests/server/security_config_api.rs

## Slice 4

- AC-18: `account_answers_the_recorded_shape` and `the_account_of_the_administrator_is_reserved` in crates/node/tests/server/security_account.rs
- AC-19: `a_user_changes_its_own_password`, `a_wrong_current_password_changes_nothing`, `a_malformed_password_change_is_400` and `the_administrator_cannot_change_a_stored_password` in crates/node/tests/server/security_account.rs; `a_password_change_needs_both_strings` in crates/coordinator/tests/it/security_info.rs
- AC-20: `a_user_without_admin_access_reads_its_own_account` in crates/node/tests/server/security_account.rs; `every_registered_route_has_an_action_row` in crates/node/tests/server/security_authz.rs

## Slice 5

Tokens are minted by `fixtures/security-jwt/mint.py` (PyJWT and `cryptography`), not by the verifier.

- AC-21: `each_key_type_authenticates_its_tokens`, `a_token_that_is_not_valid_is_401` and `the_key_decides_the_algorithm` in crates/node/tests/server/security_jwt.rs; `every_rule_refuses`, `a_lifetime_over_the_bound_is_refused` and `only_a_plain_algorithm_header_is_read` in crates/coordinator/tests/it/security_jwt.rs
- AC-22: `a_token_acts_with_the_roles_its_claim_maps_to` and `concurrent_tokens_for_one_subject_do_not_share_roles` in crates/node/tests/server/security_jwt.rs; `a_token_identity_takes_roles_only_from_its_backend_roles` in crates/coordinator/tests/it/security_jwt.rs
- AC-23: `a_token_cannot_claim_a_stored_users_name` and `a_token_does_not_run_bcrypt_or_read_the_store` in crates/node/tests/server/security_jwt.rs; `a_subject_naming_a_user_or_the_administrator_is_inadmissible` in crates/coordinator/tests/it/security_jwt.rs
- AC-24: `authinfo_and_account_answer_a_token_identity` and `securityconfig_lists_the_jwt_domain_without_its_key` in crates/node/tests/server/security_jwt.rs; `authinfo_of_a_token_identity_lists_its_roles` and `securityconfig_adds_the_domain_with_the_key_redacted` in crates/coordinator/tests/it/security_jwt.rs
- AC-25: `without_a_key_a_token_is_401` in crates/node/tests/server/security_jwt.rs, and the existing authn suite for basic

