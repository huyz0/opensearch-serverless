# Nested and reverse_nested aggregations

## Serves

Parity task R10.2 (aggregations), `nested` and `reverse_nested`; invariant I-13 (REST matches OpenSearch); ADR 0004 (bounded memory). Fourth slice of nested support (nested sorts and `inner_hits` are another thread's), after the reader's `nested` query, the mapping and child documents ([nested-fields](../nested-fields/SPEC.md), ADR 0067) and their writer.

## Change

`oss-reader` parses `nested` and `reverse_nested` as bucket aggregations and runs them on the engine's block join (lucene-rust's `AggNode::Nested` and `AggNode::ReverseNested`, ported from `NestedAggregator` and `ReverseNestedAggregator`). Each is one bucket: `{"doc_count": n, <sub-aggregations>}`.

- **`nested`** (`path`, required): the sub-aggregations run over the nested documents of `path` that belong to each document collected; `doc_count` counts those children. Its children filter is `_nested_path:<path>`; its parents filter is the non-nested documents (`_primary_term` exists), or the enclosing nested object's documents when `path` sits inside another `nested`.
- **`reverse_nested`** (`path`, optional): inside a `nested`, hands each child's parent to the sub-aggregations, once per parent; `doc_count` counts those parents. With `path`, the parent is the enclosing nested object's document of that path; without, the root document.
- **Unmapped `path`** (or a leaf field) in `nested` and `reverse_nested` gives `{"doc_count": 0}` with each sub-aggregation empty, as `NestedAggregatorFactory.Unmapped` does.
- **Refused as the Java refuses**, with its messages: a `nested` path that is mapped but not nested (`[nested] nested path [p] is not nested`, `aggregation_execution_exception`); a `reverse_nested` outside a `nested` (`Reverse nested aggregation [name] can only be used inside a [nested] aggregation`, `illegal_argument_exception`, a 400 where the others are 500); a `reverse_nested` path that is not nested (`[reverse_nested] nested path [p] is not nested`); a missing `path` for `nested` (`Missing [path] field for nested aggregation [name]`); an unknown key (`Unknown key for a VALUE_STRING in [name]: [k].`).
- **Not here:** a `top_hits` or other scoring sub-aggregation under `nested` (the engine refuses a scored aggregation under it; the node answers 400 `illegal_argument_exception` naming the aggregation), nested sorts and `inner_hits` (another thread).
- **Under a `terms`:** a `nested` or `reverse_nested` may be the sub-aggregation of a `terms` (keyword or numeric), at the top level or inside a `nested`: each bucket of the `terms` owns one nested bucket. `nested > terms > reverse_nested` (the parents of the documents with each term) and `terms > nested` are the usual shapes. Other bucket aggregations under a keyword `terms` stay refused.
- **Memory:** the parent and child lists are the engine's per-segment lists, built once a segment and reserved before the search runs (`4 * max_doc` bytes a node, [nested-memory](../nested-memory/SPEC.md) AC-5); a `nested` is one bucket, so it adds one to the bucket count.

## Acceptance criteria

- AC-1: `nested` parses its `path` and sub-aggregations; the Java's parse errors, with its messages.
- AC-2: `reverse_nested` parses an optional `path` and sub-aggregations; the same errors.
- AC-3: `nested` answers `doc_count` of the children, and sub-aggregations (`sum`, `avg`, `value_count`, `terms`, `max`) over the children's fields, on an index of blocks, with and without a query around it.
- AC-4: `reverse_nested` under `nested` answers each parent once per bucket, with and without `path`, and its sub-aggregations read the parent's fields.
- AC-5: an unmapped `nested` path answers `doc_count: 0`; a non-nested `path` and a `reverse_nested` outside `nested` fail as the Java fails.
- AC-6: the shard results reduce across shards (doc counts add; sub-aggregations reduce as they do elsewhere).
- AC-7: the aggregation tree stays within `search.max_buckets` and the depth limit; a scored sub-aggregation is refused by name.
- AC-13: `crates/reader/src/aggs/bucket/nested_agg_collect_tests.rs`, over `Spec::with_nested()` (children carry `c.k`).
- AC-14: a search with a nested aggregation reserves its bit set and lists with its hits, so a pool that holds the same search without it refuses it; and 10,000 nested documents (20,000 children) answer exactly (`doc_count`, `value_count`, `max`, `reverse_nested`) with the figures recorded as provisional.
- AC-8: through a node with `serverless.nested.enabled`: create, index nested documents, run `nested` with a `terms` and a `reverse_nested` and read the Java's bodies.
- AC-9: a `nested` under a keyword `terms` counts each bucket's own children and runs its sub-aggregations over them; the same under a numeric `terms`; shard results reduce.
- AC-10: a `reverse_nested` under a `terms` under a `nested` counts, for each term, the parents of the children holding it, with sub-aggregations over the parents' fields; a `reverse_nested` under a `terms` outside a `nested` is refused as the Java refuses it.
- AC-11: the reservation of a search counts a nested aggregation's buckets under a keyword `terms` (an owner each kept bucket).
- AC-12: the stock 3.10.0 answers recorded in `fixtures/compat/nested-search` (a nested query, nested and reverse_nested aggregations, under a `terms`, their refusals) are the node's, scores included.
- AC-13: `avg`, `max`, `min` and `value_count` over a nested numeric field (`c.k`) answer over the children of the documents collected, with and without a query, and reduce across shards.
- AC-14: `crates/reader/src/search_nested_bytes_tests.rs` (the refusal) and `crates/reader/src/aggs/bucket/nested_soak_tests.rs` (`--ignored`, by hand).

## Test plan

- AC-1, AC-2, AC-5 (parse): `crates/reader/src/aggs/bucket/nested_agg_tests.rs`.
- AC-3, AC-4, AC-6, AC-7: `crates/reader/src/aggs/bucket/nested_agg_collect_tests.rs`, over the testkit's `Spec::with_nested()` index.
- AC-9, AC-10: `crates/reader/src/aggs/bucket/nested_under_terms_tests.rs`.
- AC-11: the same file, over the reservation.
- AC-12: `crates/compat/tests/node_cluster.rs`, `an_assembled_node_answers_every_nested_fixture_as_stock_opensearch_did`.
- AC-8: `crates/node/tests/nested_aggs.rs` (a `Cluster` test, as `crates/node/tests/nested.rs`).

## S3 request budget

Unchanged: the aggregation reads the segments the query reads and no more. LIST 0, HEAD 0, PUT 0 added.

## Switch

None: an index that holds no nested object is untouched, and an aggregation that names `nested` is refused today (so the change only answers what was an error).

## Risks

- The engine's nested join is ported from Java but has seen no recorded OpenSearch answers here; expected bodies are read from the Java source until fixtures exist.
- A `nested` over a very wide block is bounded by the nested objects limit (10,000 a document).
