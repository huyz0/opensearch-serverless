# Nested and reverse_nested aggregations

## Serves

Parity task R10.2 (aggregations), `nested` and `reverse_nested`; invariant I-13 (REST matches OpenSearch); ADR 0004 (bounded memory). Fourth slice of nested support (nested sorts and `inner_hits` are another thread's), after the reader's `nested` query, the mapping and child documents ([nested-fields](../nested-fields/SPEC.md), ADR 0067) and their writer.

## Change

`oss-reader` parses `nested` and `reverse_nested` as bucket aggregations and runs them on the engine's block join (lucene-rust's `AggNode::Nested` and `AggNode::ReverseNested`, ported from `NestedAggregator` and `ReverseNestedAggregator`). Each is one bucket: `{"doc_count": n, <sub-aggregations>}`.

- **`nested`** (`path`, required): the sub-aggregations run over the nested documents of `path` that belong to each document collected; `doc_count` counts those children. Its children filter is `_nested_path:<path>`; its parents filter is the non-nested documents (`_primary_term` exists), or the enclosing nested object's documents when `path` sits inside another `nested`.
- **`reverse_nested`** (`path`, optional): inside a `nested`, hands each child's parent to the sub-aggregations, once per parent; `doc_count` counts those parents. With `path`, the parent is the enclosing nested object's document of that path; without, the root document.
- **Unmapped `path`** (or a leaf field) in `nested` and `reverse_nested` gives `{"doc_count": 0}` with each sub-aggregation empty, as `NestedAggregatorFactory.Unmapped` does.
- **Refused as the Java refuses**, with its messages: a `nested` path that is mapped but not nested (`[nested] nested path [p] is not nested`, `aggregation_execution_exception`); a `reverse_nested` outside a `nested` (`Reverse nested aggregation [name] can only be used inside a [nested] aggregation`); a `reverse_nested` path that is not nested (`[reverse_nested] nested path [p] is not nested`); a missing `path` for `nested` (`Missing [path] field for nested aggregation [name]`); an unknown key (`Unknown key for a VALUE_STRING in [name]: [k].`).
- **Not here:** a `top_hits` or other scoring sub-aggregation under `nested` (the engine refuses a scored aggregation under it; the node answers 400 `illegal_argument_exception` naming the aggregation), nested sorts and `inner_hits` (another thread), a `nested` under a keyword `terms` until that path is checked.
- **Memory:** the parent and child lists are the engine's per-segment lists, built once a segment and reserved with the aggregation's other per-segment state; a `nested` is one bucket, so it adds one to the bucket count.

## Acceptance criteria

- AC-1: `nested` parses its `path` and sub-aggregations; the Java's parse errors, with its messages.
- AC-2: `reverse_nested` parses an optional `path` and sub-aggregations; the same errors.
- AC-3: `nested` answers `doc_count` of the children, and sub-aggregations (`sum`, `avg`, `value_count`, `terms`, `max`) over the children's fields, on an index of blocks, with and without a query around it.
- AC-4: `reverse_nested` under `nested` answers each parent once per bucket, with and without `path`, and its sub-aggregations read the parent's fields.
- AC-5: an unmapped `nested` path answers `doc_count: 0`; a non-nested `path` and a `reverse_nested` outside `nested` fail as the Java fails.
- AC-6: the shard results reduce across shards (doc counts add; sub-aggregations reduce as they do elsewhere).
- AC-7: the aggregation tree stays within `search.max_buckets` and the depth limit; a scored sub-aggregation is refused by name.
- AC-8: through a node with `serverless.nested.enabled`: create, index nested documents, run `nested` with a `terms` and a `reverse_nested` and read the Java's bodies.

## Test plan

- AC-1, AC-2, AC-5 (parse): `crates/reader/src/aggs/bucket/nested_agg_tests.rs`.
- AC-3, AC-4, AC-6, AC-7: `crates/reader/src/aggs/bucket/nested_agg_collect_tests.rs`, over the testkit's `Spec::with_nested()` index.
- AC-8: `crates/node/tests/nested_aggs.rs` (a `Cluster` test, as `crates/node/tests/nested.rs`).

## S3 request budget

Unchanged: the aggregation reads the segments the query reads and no more. LIST 0, HEAD 0, PUT 0 added.

## Switch

None: an index that holds no nested object is untouched, and an aggregation that names `nested` is refused today (so the change only answers what was an error).

## Risks

- The engine's nested join is ported from Java but has seen no recorded OpenSearch answers here; expected bodies are read from the Java source until fixtures exist.
- A `nested` over a very wide block is bounded by the nested objects limit (10,000 a document).
