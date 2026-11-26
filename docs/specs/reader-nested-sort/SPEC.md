# Reader: nested sorts

## Serves

Roadmap R10.1 (what the nested spec lists as "not here"): invariant I-13 (REST matches OpenSearch), ADR 0004 (bounded memory). After [reader-nested-query](../reader-nested-query/SPEC.md) and the writer slice of [nested-fields](../nested-fields/SPEC.md).

## Change

A sort key on a field inside a nested object may name the nested object, and the parents are then ordered by the value of their children, from `FieldSortBuilder.build`, `SortBuilder.resolveNested` and `NestedSortBuilder.fromXContent` at OpenSearch 3.10.0:

- `{"c.w": {"order": "asc", "nested": {"path": "c", "filter": {...}}}}`, and the older `nested_path` and `nested_filter` that mean the same. Setting both forms fails as OpenSearch fails.
- The children are the documents of `_nested_path:<path>` that match the filter (all of them without one). A parent is ordered by the least value among them when the order is ascending and the greatest when it is descending, which is OpenSearch's default `mode`. A parent with no child that has a value is missing, and missing values are last whichever the order.
- The engine does the work: a `ToParentBlockJoinSortField` over the parents (`Queries.newNonNestedFilter`) and the children's bit sets. The keys served are keyword, `long`, `integer` and `date` fields with doc values, the same as a sort without `nested`.
- The sort value a hit reports is the child value that placed it.

Refused as not supported yet, with the node's one message: `max_children`, a `nested` inside a `nested` (more than one level), `mode`, `missing`, `numeric_type` and `format` (as before, a sort key serves `order` and the nested object), and `search_after` with a nested sort key.

Without a `nested` object a sort is exactly what it was, including a sort on a field inside a nested object, which sees the parents' own values (none).

## Acceptance criteria

- AC-1: the parser reads `nested` with `path`, `filter`, and `nested_path` and `nested_filter` into one key, and fails as OpenSearch does: both forms together (`Setting both nested_path/nested_filter and nested not allowed`), an unknown key (`malformed nested sort format, unknown field name [x]`), a value that is not an object (`malformed nested sort format, must start with an object`).
- AC-2: `max_children`, a nested `nested`, and `search_after` with a nested sort key are refused with the node's one message.
- AC-3: an ascending sort on a nested keyword field orders the parents by their least child value, a descending sort by their greatest, the total counting parents.
- AC-4: a `filter` restricts the children that count, and a parent with no matching child is last in both orders.
- AC-5: a hit reports the child value that placed it in its `sort` array.
- AC-6: a path that is not a nested object fails with `[nested] failed to find nested object under path [p]` (`query_shard_exception`); the node's mapping does not hold plain objects, so a plain object is answered the same.
- AC-7: a nested sort adds no blob request over the same search sorted without it, and a search without one is unchanged.

## Test plan

- AC-1, AC-2: `dsl/sort.rs` unit tests.
- AC-3 to AC-6: `search_nested_sort_tests.rs`, over the `Spec::with_nested` index (a keyword child `c.w`, children `w<i>-0` and `w<i>-1`).
- AC-7: by construction (the sort reads doc values of segments the search already reads); not counted by a test (NOT-RUN until the request-budget table has a nested row).

## S3 request budget

Unchanged: the sort reads doc values of segments the search already reads, and the children's bit sets come from the same segment data. No new blob request, no new sequential depth.

## Switch

None. A sort key that names a nested object was refused before, so there is no behaviour to keep behind a setting.

## Risks

A filter on a nested sort is applied as OpenSearch applies it with one level: the type filter is not added when a filter is given. A filter that matches documents outside the path would then count them as children; the engine's block join only reads the documents inside each parent's block. A `nested` sort inside `top_hits` with a filter is refused until that path carries a query context.

## Not tested

A numeric child field: the nested test index has only a keyword child, and the numeric path is the same code with another engine type.
