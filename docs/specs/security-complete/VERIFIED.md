# Security plugin, complete and native: evidence

## Acceptance evidence, slice 1

- AC-1: `a_list_holds_every_document_as_a_read_renders_it` and `an_empty_kind_lists_as_an_empty_object` in crates/node/tests/server/security_route.rs; the same names in crates/coordinator/tests/it/security_patch.rs
- AC-2: `a_list_costs_a_marker_and_a_get_a_document` in crates/node/tests/server/security_route.rs
- AC-3: `a_list_over_the_bound_is_refused_whole` in crates/node/tests/server/security_route.rs
- AC-4: `patch_applies_each_operation`, `a_patch_that_makes_an_invalid_document_changes_nothing` and `a_patch_of_the_wrong_shape_is_400` in crates/node/tests/server/security_route.rs; `a_patch_that_makes_an_invalid_document_is_refused_as_a_put_would` in crates/coordinator/tests/it/security_patch.rs
- AC-5: `a_patch_of_an_absent_document_is_404` and `a_patched_password_is_stored_as_a_hash` in crates/node/tests/server/security_route.rs
- AC-6: `a_bulk_patch_changes_several_documents`, `an_invalid_operation_in_a_bulk_patch_writes_nothing` and `a_bulk_patch_over_100_documents_is_refused` in crates/node/tests/server/security_route.rs
- AC-7: `a_patch_costs_what_a_replacing_put_costs` in crates/node/tests/server/security_route.rs
- AC-8: `listing_a_kind_is_refused_to_a_user_and_served_to_an_administrator` in crates/node/tests/server/security_authn.rs; `every_registered_route_has_an_action_row` in crates/node/src/security_authz.rs
- AC-9: `with_security_off_no_guard_is_installed` in crates/node/src/admin_assemble.rs
