# oss-meta

The metadata plane on registers: what names exist and what they hold, kept in the object store and changed only by compare-and-swap, as the Java shell's `metadata` package does, so Rust and Java nodes on one bucket agree.

## Owns
Index and alias name rules, the descriptor store (create, read, update and delete an index's descriptor register, and create, move and resolve aliases in the same namespace), the alias mutation protocol that keeps every member's `aliased_by` hint ahead of its alias and recovers a crashed writer's change, deleted names' tombstones and markers, the conditional-delete probe, the tombstone sweep, listing names a bounded page at a time, the change stripes writers confirm indexes by, the routing cache that answers routing from a read up to a second old, the cluster config register of an operator's persistent settings, and the index lifecycle: creating an index, deleting one with the reclaim intent that finishes it once a writer that missed it has stopped, and purging a deleted shard's bytes.

## Depends on
`oss-blob` (registers), `oss-format` (the records' bytes). Never a role crate.

## Entry points
`src/lib.rs`: `DescriptorStore`, `AliasMutations`, `RoutingCache`, `ChangeStripes`, `ClusterConfig`, `IndexLifecycle` and `ReclaimQueue` over any `BlobStore`, and `validate_index_or_alias`. Tested on the emulator, S3 over loopback and the filesystem store with exact request counts (`tests/descriptors/`). Spec: [docs/specs/descriptor-store](../../docs/specs/descriptor-store/SPEC.md).
