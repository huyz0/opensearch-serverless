# 0030: A booted node pushes commits, and its own reader follows its own writer

**Status:** Accepted after independent review, 2026-11-24

## Context

The commit push ([writer-push-sender](../specs/writer-push-sender/SPEC.md), [reader-push-handler](../specs/reader-push-handler/SPEC.md)) existed but a booted node never built a sender or registered a handler, so a reader that had opened a shard kept that commit for ever. The takeover fault suite then showed two more defects under the tail overlay: the node that owns a shard received none of its own writer's tail (its first search after a refreshed write missed it), and a node that took a shard over opened its reader on the dead writer's commit, which the register still named, and was refused with `shard_behind_exception` for as long as it lived.

## Decision

- The writer role tells a `CommitSink` of every commit a publish stages. The node's sink gives the commit to its own reader at once (kept for the next open if the shard is not open, so a successor opens on its own newest commit), and queues it (256, a full queue drops it) for one worker that sends it to the ranked readers.
- The own-reader take is always on. The send to other nodes is `serverless.search.commit_push.enabled` (`OSS_COMMIT_PUSH`): default on while the tail overlay is off, where it is the freshness path; default off with the overlay on, because there it was measured to make every reader drop its open shard and reopen cold on each publish (refresh=true visible p99 941.6 ms against 408.7 ms, 44 range GETs a write against none). With it off, the first publish after a takeover is still sent: readers hold a dead writer's commit and tail and learn the new writer only from it.
- The node that writes a shard follows its own writer's tail like any reader: a search of the shard asks its writer (over the transport, to itself) for the grant, and is a subscriber to its pushes. A shard that refuses a search as behind its writer is let go, so its next search opens it afresh and asks for the tail again.

## Consequences

- A push costs no object-store request; the reader's next open spends the descriptor GET.
- A search on the owner may make one transport call to its own node when it has no grant, as a remote reader does.
- Rules out sending every commit with the overlay on until measured not to cost the reader a cold open.
