# Node: the tail overlay, switched per index

## Serves

R7.6 follow-on of [node-overlay-wiring](../node-overlay-wiring/SPEC.md); [ADR 0018](../../adr/0018-tail-overlay-in-the-reader.md) says the overlay setting is a cluster default overridable per index, and principles say a feature with a measured downside is a setting overridable per index.

## Change

`index.serverless.search.tail_overlay.enabled` (index scope, default `true`) is read from the index's settings as its descriptor holds them when a shard is opened. On a node where the overlay is wired (`serverless.search.tail_overlay.enabled` true, which is the cluster default), a search of an index whose setting is `false` is served from the commit alone: the overlay source's view is not used. An unset, empty or unparseable value is `true`. The freshness check of `refresh_visibility` still applies to such an index: only the overlay is skipped.

Not in this change: turning the overlay on for one index of a node where it is not wired (a node with the setting off builds nothing, by design); a setting change reaching a shard already open (it applies when the shard is next opened).

## Acceptance criteria

- AC-1: the index setting is registered with scope index, default true, under `index.serverless.`.
- AC-2: with the overlay wired and the index setting `false`, a search answers from the commit alone; with it unset or `true` the overlay applies as before.
- AC-3: an unparseable value counts as `true`.

## Test plan

`crates/node/tests/overlay_search.rs`, one test a criterion, over the same fixture.

## S3 request budget

Unchanged: reading the setting reads nothing from the store (the descriptor is already open). LIST 0.

## Switch

The setting is the switch; default `true` only matters where the node setting, default `false`, wired the overlay.

## Risks

An index that opts out still has its overlay built and held for it by the builder (the freshness check asks the source); the cost of that is the node setting's.
