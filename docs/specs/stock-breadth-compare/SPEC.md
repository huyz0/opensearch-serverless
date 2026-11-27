# Stock OpenSearch comparison: term-level, full-text and bucket-aggregation breadth

## Serves

Roadmap R10.1 and R10.2, whose rows say stock-OpenSearch answers were not run. I-13 (REST matches OpenSearch's recorded responses).

## Change

Records new fixtures from a stock OpenSearch 3.10.0 release tarball and replays them through an assembled node. Adds `fixtures/compat/search-breadth/` (replayed) and `fixtures/compat/search-breadth-gaps/` (recorded, not replayed: what the node refuses today) and one test in `crates/compat/tests/node_cluster.rs`. No product code changes.

## Acceptance criteria

- AC-1: every term-level, full-text and compound query step in `search-breadth/queries.json` gets, from an assembled node, the status and body stock OpenSearch 3.10.0 recorded (after the listed normalisation).
- AC-2: every bucket-aggregation step in `search-breadth/bucket-aggs.json`, refusals included, gets the recorded answer.
- AC-3: every step the node does not serve yet is recorded from stock in `search-breadth-gaps/`, so each gap has the answer to match.
- AC-4: `search-breadth-gaps/` is replayed too: every step matches stock.

## Test plan

`an_assembled_node_answers_every_breadth_fixture_as_stock_opensearch_did` was run before the unserved steps were moved to `search-breadth-gaps/` and failed on 36 steps; that list is the gap list.

## S3 request budget

Unchanged; tests only.

## Switch

None; fixtures and a test.

## Risks

The fixtures hold one shard and eight documents: they pin shapes and scores, not scale.
