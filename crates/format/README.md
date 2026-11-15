# oss-format

Reads, and later writes, the blob formats the Java serverless shell defines: one immutable blob per segment with its file-offset header, node packs, and commit descriptors and manifests. Ported from the shell so Rust and Java nodes share one bucket. Pure parsing over bytes: no I/O, no S3.

## Owns
The Java shell's blob formats: segment blobs, node packs, commit descriptors.

## Depends on
`oss-types`.

## Entry points
`src/lib.rs` describes the read path: `DescriptorRecord::parse` on the descriptor register, `CommitManifest::parse` on the commit register, then `ShardRef::resolve` for every file's blob key and range. `SegmentBlobHeader` and `NodePackHeader` read the blobs' own headers. Tested against blobs the Java shell wrote (`fixtures/shell-blobs/`). Spec: [docs/specs/blob-formats](../../docs/specs/blob-formats/SPEC.md).
