# Reader soft deletes: a replaced or deleted document is not found

## Serves

Parity plan R6 exit gate: a Rust reader opening what the Rust writer published must count and find what a Java reader would. The writer marks a replaced or deleted document with 1 in the numeric doc values field `__soft_deletes` and keeps its old version in the segment (`Shard::index`, `Shard::delete`); the shell's readers open every shard through `softDeletesWrapper`.

## Change

`oss_reader::open_located` opens the commit with `DirectoryReader::soft_deletes_wrapper("__soft_deletes")`, as the writer's own reader and Lucene's `SoftDeletesDirectoryReaderWrapper` do. Counts, hits and aggregations then skip documents carrying the soft delete.

Not changed, by design and listed: the open-cost estimate, which does not count the wrapper's live-docs bits (one bit per document of a segment with soft deletes, about 0.125 bytes a document, well inside the estimate's margin over measurements).

## Acceptance criteria

- AC-1: after a document is written again and another is deleted, a search over the published commit counts only the live documents.
- AC-2: indexes with no soft-delete field (the shell's recorded indexes and the engine-built ones) open and search as before.

## Test plan

`a_replaced_or_deleted_document_is_not_found` (`crates/node/tests/push_read.rs`, a real writer publishing to a reader). AC-2 is the existing reader and node suites, which open indexes without the field.

## S3 request budget

None: the wrapper reads the doc values file the segment already has, through the same ranged reads.

## Switch

None: counting soft-deleted documents is a bug, not a behaviour.

## Risks

- The wrapper reads each segment's `__soft_deletes` doc values at open, one more range read of a file the open already touches when the field exists.
