# oss-format

Reads, and for name records writes, the blob formats the Java serverless shell defines: one immutable blob per segment with its file-offset header, node packs, and commit descriptors and manifests. Ported from the shell so Rust and Java nodes share one bucket. Pure parsing over bytes: no I/O, no S3.

## Owns
The Java shell's blob formats: segment blobs, node packs, commit descriptors, the node write log.

## Depends on
`oss-types`.

## Entry points
`src/lib.rs` describes the read path: `DescriptorRecord::parse` on the descriptor register, `CommitManifest::parse` on the commit register, then `ShardRef::resolve` for every file's blob key and range. `SegmentBlobHeader` and `NodePackHeader` read the blobs' own headers. `IndexRecord` and `Tombstone` read and write a name register's whole value, byte for byte as the shell writes it (`fixtures/jackson/`); `AliasRecord` does the same for an alias or data stream (`fixtures/aliases/`), and `ChangeStripe` for a change stripe (`fixtures/change-stripes/`). `UnitLayoutRecord` reads and writes a shard mode's unit layout as the shell writes it (`fixtures/routing/`). `ClusterSettings` reads the cluster config register as the Java's `Settings` does and writes its flat form (`fixtures/cluster-config/`, ADR 0011), and is an `oss_types::settings::Source` for the settings read from it. `DocSetMarker` reads and writes the marker register of a set of templates or pipelines as the Java's `TemplateStore` does ([spec](../../docs/specs/doc-sets/SPEC.md)). `WalRecord` and `node_log_format::{encode, decode}` read and write a node's write log, records and blobs, as the shell's `WalRecord` and `NodeLogBlob` (`fixtures/node-log/`, [spec](../../docs/specs/node-log-format/SPEC.md)). Tested against blobs the Java shell wrote (`fixtures/shell-blobs/`). Spec: [docs/specs/blob-formats](../../docs/specs/blob-formats/SPEC.md).
