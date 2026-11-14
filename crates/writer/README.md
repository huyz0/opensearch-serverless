# oss-writer

The indexing role. Appends to the node's one write log, builds segments, uploads each as one immutable blob, publishes commits.

## Owns
The per-node write log and its fencing, segment build and upload, commit publish, and pushing tail records to readers.

## Depends on
`oss-types`, `oss-blob`. Never `oss-reader`.

## Entry points
`src/lib.rs`. Built second; see [docs/roadmap.md](../../docs/roadmap.md).
