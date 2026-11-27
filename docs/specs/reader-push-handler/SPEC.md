# Reader push handler: the node takes a pushed commit

## Serves

Parity plan R6.5 (the reader takes pushes), third slice, and [ADR 0016](../../adr/0016-native-transport-and-early-revoke.md). Builds on [reader-commit-push](../reader-commit-push/SPEC.md) (the mailbox) and [reader-push-wire](../reader-push-wire/SPEC.md) (the messages).

## Change

`oss-node` gains `push` (`crates/node/src/push.rs`): `SearchService::take_push` and `PushHandler`, a transport handler for `internal:serverless/commit/push`. A push for a shard open here, with the open index's uuid, and newer than the open commit, is kept in the mailbox and the open index is let go (searches in flight keep their own handle); the next search opens on the pushed commit with one GET (the descriptor) where the register path takes two. The handler answers whether the shard is open here. `OpenIndexes` gains `peek` and `release`. The mailbox also changes from the previous slice: it is keyed by index and carries the uuid, and a held commit is replaced only by a newer one in the shell's order (higher term, then later stage of one writer, then higher Lucene generation), where the previous slice kept the later arrival.

Not changed, by design and listed: the writer's sender, the writer answering newest-commit, the reader asking it on a cold open, and the tail-records overlay.

## Acceptance criteria

- AC-1: a push newer than the open commit lets the open index go, and the next search opens on it with one GET and no LIST, HEAD or PUT, and sees the new documents.
- AC-2: a push for an index not open here is not kept and is answered as not open.
- AC-3: a push with another uuid, another shard, bytes that are no commit, or a commit not newer than the open one leaves the open index in place and makes no request.
- AC-4: the handler answers the open flag in the shell's wire form and refuses a message that does not decode.
- AC-5: a mailbox keeps the newer commit in the shell's order whichever arrives first.
- AC-6: peeking at an open index does not count as a search; releasing one lets the next search open afresh.

## Test plan

`crates/node/tests/server/push_read.rs` (a real writer publishing, a reader over the same files), `crates/node/src/open_indexes.rs` unit tests, `crates/reader/tests/it/commit_push.rs`.

## S3 request budget

A push costs no request. The open it leads to costs one GET instead of two. Idle indexes make none.

## Switch

None: a push only matters to an index open here, and moving to a newer commit has no downside.

## Risks

- A push for a shard not yet open is dropped, so its first open reads the register (two GETs), as before.
