# Security configuration API: evidence

- AC-1: `an_assembled_node_answers_every_security_management_fixture_as_the_stock_plugin_did` in crates/compat/tests/node_cluster.rs (the five management fixtures recorded from the stock plugin 3.10.0), and the validation and rendering checks in crates/coordinator/tests/security.rs
- AC-2: `a_password_is_stored_only_as_its_hash` and `an_answer_never_carries_a_hash_or_a_password` in crates/node/tests/server/security_route.rs; `an_answer_never_carries_a_hash_or_a_password` in crates/coordinator/tests/security.rs
- AC-3: `a_mapping_for_an_absent_role_is_refused` in crates/node/tests/server/security_route.rs
- AC-4: `security_requests_per_operation` in crates/node/tests/server/security_route.rs
- AC-5: `a_password_is_stored_only_as_its_hash` in crates/node/tests/server/security_route.rs
- AC-6: `the_1001st_document_of_a_kind_is_refused` in crates/node/tests/server/security_route.rs
- AC-7: `security_is_off_by_default_and_set_by_its_variable` in crates/node/src/boot/config/tests.rs; the management replay run with the switch off fails every step with `no handler found`, observed once
