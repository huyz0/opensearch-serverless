# oss-format

Reads, and later writes, the blob formats the Java serverless shell defines: one immutable blob per segment with its file-offset header, node packs, and commit descriptors and manifests. Ported from the shell so Rust and Java nodes share one bucket. Pure parsing over bytes: no I/O, no S3.

## Owns
The Java shell's blob formats: segment blobs, node packs, commit descriptors.

## Depends on
`oss-types`.

## Entry points
`src/lib.rs`. Readers land in stage 2, tested against blobs the Java shell wrote (`fixtures/shell-blobs/`).
