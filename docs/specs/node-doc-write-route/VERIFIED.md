# Verified: a writer node serves the single-document write APIs

- AC-1: `a_document_is_created_then_updated` in crates/node/tests/server/doc_route.rs
- AC-2: `a_post_without_an_id_generates_one` in crates/node/tests/server/doc_route.rs
- AC-3: `create_refuses_a_live_id` in crates/node/tests/server/doc_route.rs
- AC-4: `delete_answers_deleted_then_not_found` in crates/node/tests/server/doc_route.rs
- AC-5: `refresh_and_if_seq_no_follow_their_parameters` in crates/node/tests/server/doc_route.rs
- AC-6: `a_refused_document_and_an_unknown_index_answer_their_failure` in crates/node/tests/server/doc_route.rs
- AC-7: `a_generated_id_carries_the_time_of_the_write` and `op_type_create_refuses_a_live_id` in crates/node/tests/server/doc_route.rs
- AC-8: `a_document_is_charged_to_the_request_body_pool_once` and `a_document_with_no_declared_length_is_charged_as_it_is_read` in crates/node/tests/server/doc_route.rs
- AC-9: `each_outcome_has_its_status`, `a_written_item_is_the_single_document_answer` and `a_failed_item_is_its_error_and_status` in crates/writer/tests/bulk_doc.rs
- AC-10: `require_alias_changes_the_unknown_index_refusal` in crates/node/tests/server/doc_route.rs
