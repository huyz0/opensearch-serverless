# A held shard adopts a changed mapping

## Serves

[PUT /{index}/_mapping](../node-index-mapping-put/SPEC.md): the Java's reconciler refresh hands a changed mapping to the shards a node holds. Without it a writer keeps the mapping it opened with, parses a document with a field the update mapped against the old one (a string for a `keyword` field becomes `text`), and its growth is then refused as a conflict with the descriptor, on every write, until the shard reopens.

## Change

The incarnation check a writer makes before a group (at most once a fence window per index, one descriptor read) already reads the descriptor. `Incarnation` gains `observe`, whose default is `live` with no mapping; `DescriptorIncarnation` answers the live flag and the stored mapping from the same read. When the mapping differs from the one the slot last adopted, the slot's shard merges the descriptor's mapping into its own (`Shard::adopt_mapping`): fields the descriptor holds that the shard lacks are added, what the shard grew and has not yet written to the descriptor stays, and a merge the mapping refuses leaves the shard's mapping as it was and is retried at the next change.

No request is added: the read is the incarnation check's own.

## Acceptance criteria

- AC-1: a shard adopts a field a changed descriptor mapping holds, and parses the next document against its type.
- AC-2: what a shard grew and has not yet written to the descriptor is kept when it adopts.
- AC-3: a descriptor mapping that conflicts with the shard's leaves the shard's mapping as it was.
- AC-4: on a node, a write after an update that mapped a field `keyword` is indexed as `keyword`, and costs no request the incarnation check did not already make.

## Test plan

`crates/writer/tests/shard_growth.rs` for AC-1 to AC-3; `crates/node/tests/dynamic_mapping.rs` for AC-4, with the request meter.

## S3 request budget

None added: one descriptor read a fence window, as before.

## Switch

None: the Java does it unconditionally.
