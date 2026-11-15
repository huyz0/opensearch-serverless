# Reader: open a published commit

## Serves

Roadmap stage 2, its first slice; invariant I-18 (new); ADR 0002 (lucene-rust as a pinned git dependency).

## Change

`oss-reader` finds a shard's latest commit from the descriptor and commit registers (`ShardFiles`). It opens the commit with lucene-rust's `CachedBlobDirectory` over `EngineStore`, a read-only lucene-rust `BlobStore` that maps each Lucene file to a range of its shell blob. lucene-rust comes in pinned at one revision. Searching, tail records and REST come in later changes.

## Acceptance criteria

- AC-1: the commit the Java shell published is found with exactly two GETs and nothing else, and each file maps to the blob the shell put it in (node pack or segment blob).
- AC-2: the engine opens both segments of that commit (5 documents), with no LIST, HEAD or write. Every read other than the two registers is a ranged GET.
- AC-3: lengths, versions and existence come from the manifest with no request; writes are refused.
- AC-4: reading a whole file is one ranged GET of exactly that file's bytes.
- AC-5: a missing index fails as not found after one GET.

## Test plan

Integration tests in `crates/reader/tests/it/` over `fixtures/shell-blobs` loaded into the emulator, asserting exact request counts.

## S3 request budget

Open: 2 GET (registers), then ranged GETs for the files the engine reads at open. LIST 0, HEAD 0, PUT 0.

## Switch

None yet: the reader is not wired into a node.

## Risks

A manifest without file lengths (older shells) is refused (`UnknownLength`); a later change can read lengths from blob headers.
