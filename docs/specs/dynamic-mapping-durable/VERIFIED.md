# Dynamic mapping reaches the descriptor: verification

- AC-1: `a_shard_hands_over_what_its_writes_added_to_the_mapping` (writer, `tests/shard.rs`).
- AC-2: `a_range_query_on_a_dynamically_mapped_field_finds_every_document_on_every_node` (node, `tests/dynamic_mapping.rs`); seen failing (timeout, range finds 0 while match-all finds 20) with the writer role's growth removed.
