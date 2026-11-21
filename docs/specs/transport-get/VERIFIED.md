# Verified: transport-get

- AC-1: `a_get_request_is_the_task_id_index_uuid_shard_and_id` in crates/transport/tests/wire/read.rs
- AC-2: `a_get_response_is_owner_id_found_source_sequence_identity_and_routing` in crates/transport/tests/wire/read.rs
- AC-3: `a_get_response_for_a_missing_document_carries_the_unassigned_identity` in crates/transport/tests/wire/read.rs
- AC-4: `the_get_action_name_is_the_javas` in crates/transport/tests/wire/read.rs
