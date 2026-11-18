# The node searches over an installed overlay source

## Serves

Parity plan R7.6, the tail overlay engine, fifth piece ([ADR 0018](../../adr/0018-tail-overlay-in-the-reader.md)). Builds on [reader-overlay-search](../reader-overlay-search/SPEC.md). The background builder over `TailHeld`, the pool charge and the fetch on open are later pieces; no source is installed by a node yet.

## Change

- `oss-reader`: `overlay::Ready` (a finished overlay with the commit's copies of its ids) and the `OverlaySource` trait, `ready(index_uuid, shard, stage, commit, now_millis) -> Option<Arc<Ready>>`: only a finished overlay, never started by a search.
- `oss-node`: the setting `serverless.search.tail_overlay.enabled` (node scope, default `false`), `tail_overlay::install` (as `newest_ask::install`) and `SearchService::set_overlay_source`. With a source installed, `_search` serves a shard through the view the source's `Ready` makes over the open commit; the view is cached with the open shard and rebuilt only when the source returns another `Ready`. A refused view serves the commit alone.
- The index-level override `index.serverless.tail_overlay` is not defined here: the descriptor reader does not yet read an index's settings. The next piece adds both together.

## Acceptance criteria

- AC-1: with the setting off no source is installed, and a search is answered from the commit alone.
- AC-2: with a source installed a search finds the overlay's documents and not the commit's copies they replace.
- AC-3: a source that has no overlay yet leaves the answer the commit's.
- AC-4: a search does not rebuild the view while the source returns the same `Ready`: one view serves the searches between two overlays.
- AC-5: the setting is `false` by default and the node's registry holds it.

## Test plan

`crates/node/tests/overlay_search.rs`, one test per criterion, over a commit a writer published to a file store, with a test source that builds its `Ready` from the commit it is given. AC-5 is in `crates/node/src/settings.rs`' registry test.

## S3 request budget

Zero added by a search: the view reads the commit through the open shard's directory and cache. A test asserts the meter around searches with the source installed.

## Bounds

The cached view is one reader of reference counts and masks (spec reader-overlay-search, "Bounds"); the wiring piece charges it.

## Switch

`serverless.search.tail_overlay.enabled`, default `false`: on only when the overlay's search cost and write-p99 effect are measured (ADR 0018). Tests pin it off for the existing suites and on for the new.

## Risks

A source returning a `Ready` built over another commit would hide the wrong documents: the source is handed the commit and its stage, and `Ready` is keyed by them in the builder piece.
