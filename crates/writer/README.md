# oss-writer

The indexing role. Appends to the node's one write log, builds segments, uploads each as one immutable blob, publishes commits.

## Owns
The per-node write log and its fencing, segment build and upload, commit publish, and pushing tail records to readers.

## Depends on
`oss-types`, `oss-blob`, `oss-format`. Never `oss-reader`.

## Entry points
`src/lib.rs`; `log::NodeLogStore`, one node's write log by name: appends (hedged or not), fences, header and slice reads, deletes; `log::NodeLogCommitter`, group commit of every shard's records into one PUT a window, with epochs in the lease and trimming. Built second; see [docs/roadmap.md](../../docs/roadmap.md).
