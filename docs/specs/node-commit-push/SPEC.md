# Node: a published commit reaches the readers that hold the shard open

## Serves

Parity plan R9 (freshness) and [ADR 0016](../../adr/0016-native-transport-and-early-revoke.md). The commit push of R6.5 ([writer-push-sender](../writer-push-sender/SPEC.md), [reader-push-handler](../reader-push-handler/SPEC.md), [writer-push-link](../writer-push-link/SPEC.md)) was merged but never wired into a booted node: no sender was built and no handler registered, so a reader that had opened a shard kept that commit for ever. A booted three-node cluster with the tail overlay off showed a write made after the first search invisible on every node, the owner's included, 36 s later. This wires it.

## Change

- `WriterRole` gains a `CommitSink` (set once, like the tail sink): every publish that stages a shard's commit ([`WriterRole::staged`]) tells it the index name, uuid, shard and the commit bytes, after the role's locks are released.
- A booted node's sink (`oss-node` `commit_push`) does two things with each commit:
  - the node's own reader takes it at once (`SearchService::take_push`): the node that owns a shard also serves searches, and nothing pushes to oneself;
  - the commit goes to a bounded queue (256) read by one worker thread that sends it with the `PushSender` to the readers placement ranks for the shard, other than this node. A full queue drops the commit: a lost push costs nothing, the reader's next open reads the register.
- A booted node registers the `PushHandler` for `internal:serverless/commit/push`, behind the node's transport signature check as the tail's handlers are.
- `serverless.search.commit_push.enabled` (node scope, `OSS_COMMIT_PUSH`) turns the send to other nodes on or off. The own reader's local take is not part of it: it has no cost and an owner that never took its own commit served a dead writer's. Default true while the tail overlay is off, false when it is on (see Switch).

Two defects of the owner's own searches under the overlay are fixed with it (found by the takeover suite): the owner's reader never received its own writer's tail, and a node that took a shard over opened its reader on the dead writer's commit and then asked that writer for a grant the live one could not give (a 503 `shard_behind_exception` for ever).

Not changed: pushes carry no inline blocks, the writer answers no newest-commit ask (no handler is registered for it), the tail overlay.

## Acceptance criteria

- AC-1: three booted nodes, tail overlay off: a shard opened by a search on every node, then written again and refreshed on its owner, is counted with the new documents by every node within 10 s.
- AC-2: the owner's own search is among them (the sink's local take).
- AC-3: with `OSS_COMMIT_PUSH=false` a node sends nothing: the second write stays invisible on the other nodes' opened shard, as before; the owner's own reader still takes it.
- AC-4: the sink holds no object store: a push is a message between nodes, and the reader's next open spends the one descriptor GET of [reader-push-handler](../reader-push-handler/SPEC.md)'s budget row.
- AC-5: the sink's queue is bounded: a full queue drops the commit and counts it; the publishing call never waits for a send.
- AC-6: `OSS_COMMIT_PUSH` defaults to true with the overlay off and false with it on, an explicit value wins, and one that is not true or false is refused.
- AC-7: with the tail overlay on, a node that owns a shard counts a refreshed write on its first search, as every other node does: the owner's reader follows its own writer's tail (records its writer acknowledged here reach its own overlay, and a search of the shard asks its writer like any reader's does).
- AC-8: a commit this node's writer publishes while no search holds the shard open is kept for the next open, so a node that took a shard over opens on its own newest commit, not on the dead writer's the register still names.
- AC-9: three nodes, overlay on, the owner killed with both readers holding its tail: no search that reports no shard failure counts fewer documents than were acknowledged, none counts more, and after the takeover every acknowledged write and update is counted exactly once on each survivor, a refreshed write from its first search.

## Test plan

`crates/node/tests/commit_push_wired.rs` over the cluster rig (AC-1 to AC-3, both settings); `crates/node/tests/overlay_takeover.rs` (AC-7 to AC-9); unit tests of the sink in `crates/node/src/commit_push.rs` (AC-5) and `NodeConfig` (AC-6).

## S3 request budget

The push is between nodes. The reader that opens on a pushed commit spends one GET (the descriptor) where an open from the register spends two ([search_open](../reader-push-handler/SPEC.md)); that row holds it.

## Switch

A setting (build rule): the send costs no object-store request, but with the tail overlay on it was measured to hurt (VERIFIED.md: refresh=true visible p99 941 ms against 418 ms, 44 range GETs a write against none, because each publish made every reader drop its open shard and reopen it cold while the overlay already served what it held). So the default is on only where the push is the freshness path, the overlay off, and off with the overlay on; an operator can set either.
