# oss-meta

The metadata plane on registers: what names exist and what they hold, kept in the object store and changed only by compare-and-swap, as the Java shell's `metadata` package does, so Rust and Java nodes on one bucket agree.

## Owns
Index and alias name rules, the descriptor store (create, read, update and delete an index's descriptor register), deleted names' tombstones and markers, the conditional-delete probe, the tombstone sweep, listing names a bounded page at a time, and the change stripes writers confirm indexes by. Later: cluster config.

## Depends on
`oss-blob` (registers), `oss-format` (the records' bytes). Never a role crate.

## Entry points
`src/lib.rs`: `DescriptorStore` over any `BlobStore`, and `validate_index_or_alias`. Tested on the emulator, S3 over loopback and the filesystem store with exact request counts (`tests/descriptors/`). Spec: [docs/specs/descriptor-store](../../docs/specs/descriptor-store/SPEC.md).
