# Writer push sender: tell the shard's readers of a new commit

## Serves

Parity plan R6.5 (the reader takes pushes), fourth slice, and [ADR 0016](../../adr/0016-native-transport-and-early-revoke.md). Sends what [reader-push-wire](../reader-push-wire/SPEC.md) defines to the handler of [reader-push-handler](../reader-push-handler/SPEC.md).

## Change

`oss-node` gains `push_sender` (`crates/node/src/push_sender.rs`, `writer` feature): `PushSender::published` sends the commit register value a publish wrote to the readers placement ranks first for the shard, other than this node, each in parallel, and keeps which of them answered that they have the shard open. Who the readers are (`PushTargets`) and how a push reaches one (`PushLink`) are traits, so the sender is tested without sockets; the transport-backed link and the placement-backed targets are the next slice.

A push is sent after the publish, off the write path, and a lost one costs nothing: the reader's open reads the register. A failed send is counted and not retried. The map of readers with the shard open is bounded at 4,096 shards, dropping other shards' entries past it.

Not changed, by design and listed: the inline blocks a push may carry (empty here), the writer answering newest-commit, the reader asking it, the tail overlay.

## Acceptance criteria

- AC-1: a published commit is sent once to each target, with the index name, uuid, shard, the commit bytes as published, and the descriptor's mapping and settings versions.
- AC-2: a target that fails does not stop the others, and is counted as a failure; the others are counted as sent.
- AC-3: with no targets, or with pushing off, nothing is sent.
- AC-4: the readers that answered open are remembered per shard and forgotten when one answers not open.
- AC-5: the remembered shards stay within the bound.

## Test plan

Unit tests in `crates/node/src/push_sender.rs` over recording fakes of both traits.

## S3 request budget

None: a push goes between nodes and touches no store.

## Switch

`enabled` on the sender, on by default as the shell's `serverless.search.commit_push.enabled`: the push costs no object-store request and moves a reader forward at once.

## Risks

- A slow reader holds the sending call for the link's timeout; the caller sends after the publish, off the write path.
