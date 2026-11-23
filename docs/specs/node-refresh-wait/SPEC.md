# A refresh waits for the search nodes instead of publishing a commit

## Serves

Parity plan R9.3: the shell's `NodeWritePath.refresh` refreshes the engine and then waits, bounded, until the nodes that search the shard hold every acknowledged record (`RefreshVisibility` `serving` and `everywhere`); it publishes nothing. The Rust `refresh` published a commit on every `refresh=true`. Builds on [node-boot-tail](../node-boot-tail/SPEC.md) and [writer-tail-push](../writer-tail-push/SPEC.md).

## Change

- `WriterRole::set_refresh_visibility` takes a `RefreshVisibility` (`owner`, `serving`, `everywhere`; default `owner`), and `TailSink::await_visible(uuid, shard, grants)` waits for the search nodes (default: false, nothing waited for).
- `WriterRole::refresh`, after the engine refresh and with the shard lock released, calls the sink when the visibility waits. A confirmed wait returns with no publish; one that does not confirm, no sink, or `owner` publishes as before.
- `Sink::await_visible` in the booted node is `TailPush::await_search_nodes` for `WAIT` (500 ms), covering unreachable grants under `everywhere`.
- `boot` sets the mode from `serverless.search.refresh_visibility`; the reader's grant check is the same mode's `grants()`.

- A publish takes its commit's files as a snapshot under the shard's lock (a reference to each file's bytes, no copy) and uploads after the lock is let go, so a refresh or write is not held behind the lazy publish's blob uploads (100-300 ms on the latency profile). A gate held from the snapshot to the end of the upload publishes commits in the order they were taken. Without it a confirmed refresh, which no longer publishes, met the lazy publish's lock in about one refresh of six and the `refresh=true` visible p99 passed the freshness meter's bar (705-766 ms against 555).

## Acceptance criteria

- AC-1: under `serving`, a refresh the sink confirms publishes no commit (no PUT, no staged manifest), the engine refreshed, and the sink was asked once for the shard without grants.
- AC-2: under `everywhere` the sink is asked with grants.
- AC-3: a wait that does not confirm falls back to publishing a commit.
- AC-4: under `owner` the sink is never asked and the refresh publishes.
- AC-5: with no sink installed, a waiting visibility publishes.

- AC-6: a refresh made while a publish is uploading returns without waiting for the upload, and the publish still completes.
- AC-7: a snapshot lists, opens and measures exactly the files it was taken of, keeps a file the engine deletes afterwards, and refuses every write.

## Test plan

`crates/node/tests/tail/refresh_wait.rs`, a role over the blob emulator with a fake sink (AC-6 slows the emulator's PUTs), and the unit tests in `crates/node/src/writer/snapshot.rs`; the freshness meter (`crates/node/tests/freshness_meter.rs`, slow tier) is the end-to-end check.

## S3 request budget

A confirmed wait removes the blob PUT and the register swap of the refresh: PUT 0 where it was 2; LIST 0, HEAD 0. A refresh that falls back costs what it did. The default stays `owner` in the role; a node boots with the overlay off unless its setting is on, so no booted node changes until the cost and visibility fixtures are measured.

## Risks

- A refresh answered on a confirmed wait leaves the commit older than the acknowledged writes; readers not pushed to (a new reader) find them in the overlay only after asking the writer, which is what `everywhere` and its grants cover. Under `serving` a reader outside the pushed set can miss a refreshed write until the next publish.
