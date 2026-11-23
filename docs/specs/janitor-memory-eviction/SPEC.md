# Janitor memory eviction: forget the shards a node no longer holds

## Serves

R8.4, invariant I-4 (bounded memory) and the safety argument of [janitor-gc](../janitor-gc/SPEC.md): what a sweep remembers (the blobs seen unreferenced, and the scope of [janitor-sweep-scope](../janitor-sweep-scope/SPEC.md)) is held for a shard only while this node holds the shard. `Memory` and the pass's record of the commit each shard was last swept at grew with every shard a node ever held, and a shard held again after a release started with what an earlier owner-ship had seen, which is the one assumption the grace rules out.

## Change

- `oss_janitor::gc::Memory::retain(keep)` drops the watched blobs and the scope of every shard path `keep` refuses, and `Memory::scopes()` counts the shards it holds a scope for.
- `oss_node::writer::JanitorPass::run` evicts, before it judges what is due, the memory and the swept-commit record of every shard the role no longer holds, so a shard that is held again starts empty and is swept as a new owner's. It costs no request.
- `JanitorPass::remembered()` reports how many shards it holds a swept commit for and how many `Memory` holds a scope for.

A shard whose index was deleted is no longer held once the role lets it go, so its entries leave at the next pass.

## Acceptance criteria

- AC-1: `Memory::retain` keeps the entries of the paths it accepts and drops the rest, in both the watched blobs and the scopes.
- AC-2: a path evicted is swept as a new owner's: its first sweep lists, deletes nothing, and watches what it finds.
- AC-3: a pass over a role that no longer holds a swept shard forgets that shard, in the swept record and in `Memory`, and makes no request.
- AC-4: a shard still held keeps its entries across passes: a rested shard costs nothing, as before.

## Tests

`crates/janitor/tests/gc_scope.rs` (AC-1, AC-2) and `crates/node/tests/roles/writer_janitor.rs` (AC-3, AC-4).

## S3 request budget

None: eviction reads memory.

## Risks

- A shard that is released and taken again quickly forgets its grace and waits for two more sweeps and the minimum age before a blob goes: storage for longer, never a record.
