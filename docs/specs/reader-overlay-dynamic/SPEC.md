# The tail overlay builds over an index whose fields are dynamic

## Serves

Parity plan R9 (tail overlay in a booted node), found by the freshness meter ([node-freshness-meter](../node-freshness-meter/SPEC.md)). Builds on [reader-overlay-segment](../reader-overlay-segment/SPEC.md) and [mapping-growth](../mapping-growth/SPEC.md). Request-cost rule: no blob request is added.

## Change

`oss-reader`'s overlay build (`Overlay::build_over`, `build_ready`) reads a record against the mapping the shard opened with. That mapping is the descriptor's at open: empty for an index created without mappings, and without the fields dynamic mapping added since. Today the build refuses ("the mapping is empty", then "no mapper for field"), so a reader serves only published commits.

- An empty mapping source is the default (empty) mapping, as `Mapping::from_descriptor(None)` is.
- Records are read in order and each record's dynamic additions (`ParsedDocument::grown`) are carried to the next record and to indexing it, exactly as the writer grows its mapping per document. The grown mapping lives only in the build: nothing is written to the descriptor and no request is made.
- A record the mapping refuses (a conflict, `dynamic: strict`) still fails the build as today, and the shard falls back to its commit.

`oss-node`'s freshness check (`search_overlay::fresh_check`) registers the shard with the overlay source before it asks the writer (AC-6); it makes no request of its own.

Not changed: the mapping a search translates queries against (the shard's at open), the writer's mapping growth, the write route.

## Acceptance criteria

- AC-1: a build over an empty mapping source with records whose fields are all new builds, and a search of the overlay finds each record by id.
- AC-2: a later record using a field an earlier record added dynamically builds, and a field first seen in two records with the same type is one field.
- AC-3: a dynamic field whose records disagree on type refuses the build with the record's id, as today.
- AC-4: in a booted three-node cluster, an index created with no mappings is searched on another node and counts a write within 3 s of its ack, well before the 10 s publish age, so it was served by the overlay.
- AC-6: a reader whose first search of a shard comes after its writer has answered writes the commit lacks registers the shard with the overlay source before the writer is asked, so it subscribes, takes the kept records and answers; before, it asked only how far the writes went and refused every search (`shard_behind_exception`) for as long as the writes stayed unpublished. AC-4's fixture reproduces it: three writes land before the readers open.
- AC-7: a `refresh=true` write to that index is counted by the first search of each other node after its ack.
- AC-5: the build makes no store request: it takes no store, and AC-4's index sends no more requests per write than an index with an explicit mapping, within 10%.

## Test plan

AC-1 to AC-3: `crates/reader/tests/overlay_dynamic.rs`, red first (they fail on today's refusal). AC-4, AC-5: `crates/node/tests/overlay_dynamic.rs`, `#[ignore = "slow"]` on the suite rig (boots three nodes).

## S3 request budget

Unchanged: the build is memory only. LIST 0, HEAD 0, PUT 0.

## Switch

None: a refusal turned into a result; no downside and the overlay itself is switched by `OSS_TAIL_OVERLAY`.

## Risks

- A search on a dynamic field the shard's open mapping lacks still sees it as unmapped; AC-4 searches by id. A follow-up if it matters.
- Growth per record clones the mapping when a record adds fields; bounded by the field limit and the record cap.
