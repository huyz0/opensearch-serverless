# Reader: nested inner_hits

## Serves

Roadmap R10.1 (the "not here" of [reader-nested-query](../reader-nested-query/SPEC.md)): invariant I-13 (REST matches OpenSearch), ADR 0004 (bounded memory). After [reader-nested-sort](../reader-nested-sort/SPEC.md).

## Change

A `nested` query may carry `inner_hits`, from `NestedQueryBuilder`, `InnerHitBuilder.fromXContent` and `InnerHitsPhase` at OpenSearch 3.10.0. Each returned parent then carries `inner_hits.<name>.hits` with the children that matched, most relevant first:

```json
"inner_hits": {"c": {"hits": {"total": {"value": 2, "relation": "eq"}, "max_score": 1.0,
  "hits": [{"_index": "i", "_id": "1", "_nested": {"field": "c", "offset": 1}, "_score": 1.0, "_source": {...}}]}}}
```

- The name is `name` when given, else the path. `size` (default 3), `from` (default 0) and `sort` are read; `from + size` is bounded by `index.max_inner_result_window` (100), as `top_hits` is.
- A child's `_nested.offset` is its position among the parent's children of that path (its doc id less the first child's), and its `_source` is the matching sub-tree of the parent's `_source`, filtered by `_source` when given.
- The children are collected after the parent hits are chosen, from the segment's block layout: the inner query matched against the parent's own children only. A parent's work is bounded by `from + size` children held at a time.

Not in the first slice, refused by name: `docvalue_fields`, `highlight`, `explain`, `script_fields`, `version`, `seq_no_primary_term` and `inner_hits` inside an `inner_hits`; `_name` on the query; `inner_hits` on a nested query that is not at the top of `must` or `filter` of the request (one the engine cannot tie to a returned parent).

## Acceptance criteria

- AC-1: the parser reads `name`, `from`, `size`, `sort`, `_source` and refuses the rest by name; a `from + size` over the window fails `illegal_argument_exception`.
- AC-2: a parent returned by a nested query with `inner_hits` carries its matching children under the name, the total counting all matching children.
- AC-3: `size` and `from` page the children; the total does not change.
- AC-4: `_nested.offset` is the child's position among the parent's children of that path, and `_source` its sub-tree.
- AC-5: a parent without matching children (a `must_not`) carries an empty list; a search without `inner_hits` is byte-for-byte unchanged.
- AC-6: `inner_hits` adds no blob request over the search without it.

## S3 request budget

Unchanged: the children are read from segments and stored fields the search already opens; a parent's `_source` is already fetched for the hit.

## Switch

None: the key was refused before.

## Risks

The `_source` sub-tree must be cut without parsing the whole parent `_source` into memory: it streams through the same chunked parser as `_source` filtering, and the cut is bounded by the parent's `_source` length, which the hit already holds.
