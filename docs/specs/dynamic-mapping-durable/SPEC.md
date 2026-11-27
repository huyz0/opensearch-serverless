# Dynamic mapping reaches the descriptor

## Serves

Parity plan R6.4 and the fencing rule. Finishes [mapping growth](../mapping-growth/SPEC.md): `MappingGrowth` existed but no write called it, so a field a document introduced lived only in the writing shard's memory. A node that served the published segments, Rust or Java, had no mapping for the field: a range or term query on it found nothing while match-all found every document, and `_mapping` came back empty (seen in the mixed-fleet wide soak).

## Change

- `Shard::take_growth` answers what the documents applied since the last call added to the shard's mapping (the merge of their dynamic updates), or none.
- After a group's writes are applied and before they are logged, the writer role grows the index's descriptor with that addition (`MappingGrowth::grow`). If the descriptor cannot hold it, the group's written items are refused as unavailable, the log is not written, and the shard is released, so it reopens from the log and the descriptor.
- A node that holds no growth service (the role built without one) behaves as before.

## Acceptance criteria

- AC-1: the shard hands over exactly what its writes added, merged across writes, and nothing once taken; a write adding nothing hands over none.
- AC-2: on a node cluster, a bulk of documents with a field the index's mapping lacks makes a range query on that field find every document on the writing node and on another node.

## Test plan

`crates/writer/tests/it/shard.rs` (AC-1); `crates/node/tests/dynamic_mapping.rs` over the emulated cluster (AC-2). The refusal path (a descriptor that cannot hold the addition) reuses the failed-commit path and has no criterion of its own.

## S3 request budget

One register read and one swap per group that adds a field, none for a group that adds none; bounded by the field limit, as in the mapping growth spec. Nothing scales with documents or time.

## Switch

None: the Java does this unconditionally, and a field a reader cannot map is wrong output, not a trade-off.
