# Verified: writer-routing-stored

- AC-1: `a_routed_document_returns_its_routing_and_one_indexed_without_clears_it` in crates/writer/tests/it/shard_get.rs
- AC-2: `a_replayed_record_keeps_its_routing` in crates/writer/tests/it/shard_get.rs
- AC-3: `a_get_returns_the_routing_a_document_was_indexed_with` in crates/node/tests/server/get_route.rs
- AC-4: `a_forwarded_get_returns_the_routing_the_document_was_indexed_with` in crates/node/tests/server/read_forward.rs
