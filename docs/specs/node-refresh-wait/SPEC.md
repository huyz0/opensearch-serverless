# A refresh waits for the search nodes instead of publishing a commit

## Serves

Parity plan R9.3: the shell's `NodeWritePath.refresh` refreshes the engine and then waits, bounded, until the nodes that search the shard hold every acknowledged record (`RefreshVisibility` `serving` and `everywhere`); it publishes nothing. The Rust `refresh` published a commit on every `refresh=true`. Builds on [node-boot-tail](../node-boot-tail/SPEC.md) and [writer-tail-push](../writer-tail-push/SPEC.md).

## Change

- `WriterRole::set_refresh_visibility` takes a `RefreshVisibility` (`owner`, `serving`, `everywhere`; default `owner`), and `TailSink::await_visible(uuid, shard, grants)` waits for the search nodes (default: false, nothing waited for).
- `WriterRole::refresh`, after the engine refresh and with the shard lock released, calls the sink when the visibility waits. A confirmed wait returns with no publish; one that does not confirm, no sink, or `owner` publishes as before.
- `Sink::await_visible` in the booted node is `TailPush::await_search_nodes` for `WAIT` (500 ms), covering unreachable grants under `everywhere`.
- `boot` sets the mode from `serverless.search.refresh_visibility`; the reader's grant check is the same mode's `grants()`.

## Acceptance criteria

- AC-1: under `serving`, a refresh the sink confirms publishes no commit (no PUT, no staged manifest), the engine refreshed, and the sink was asked once for the shard without grants.
- AC-2: under `everywhere` the sink is asked with grants.
- AC-3: a wait that does not confirm falls back to publishing a commit.
- AC-4: under `owner` the sink is never asked and the refresh publishes.
- AC-5: with no sink installed, a waiting visibility publishes.

## Test plan

`crates/node/tests/refresh_wait.rs`, a role over the blob emulator with a fake sink.

## S3 request budget

A confirmed wait removes the blob PUT and the register swap of the refresh: PUT 0 where it was 2; LIST 0, HEAD 0. A refresh that falls back costs what it did. The default stays `owner` in the role; a node boots with the overlay off unless its setting is on, so no booted node changes until the cost and visibility fixtures are measured.

## Risks

- A refresh answered on a confirmed wait leaves the commit older than the acknowledged writes; readers not pushed to (a new reader) find them in the overlay only after asking the writer, which is what `everywhere` and its grants cover. Under `serving` a reader outside the pushed set can miss a refreshed write until the next publish.
