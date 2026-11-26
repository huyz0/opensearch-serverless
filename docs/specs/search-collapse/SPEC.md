# `_search` `collapse`

## Serves

`collapse` on the refusal list of `docs/compatibility.md`. Stock keeps the best hit of each distinct value of one keyword or number field (`CollapsingTopDocsCollector`, a first-pass grouping collector) and adds the field's value to each hit's `fields`.

## Change

- `collapse` parses as `{"field": f}` with `max_concurrent_group_searches` (a positive int, accepted and unused: it only bounds the `inner_hits` expansion). `inner_hits` under it is refused as not served.
- The shard collects candidate hits in the search's order (score, or its `sort`), reads each hit's collapse value from doc values, and keeps the first hit of each distinct value, a document without a value being one group of its own. The groups are in the order of their best hit, `from` is dropped after, and `size` of them answer. `hits.total` still counts matching documents, as stock does.
- The candidate window starts at four times `from + size` and grows by four while groups are missing and matches remain, to at most 100,000 candidates; a search needing more than that is 400 rather than unbounded work. A search that finds all its groups in the first window collects no more than it did before.
- Each hit's `fields` carries the collapse field, as `docvalue_fields` of it would.
- A keyword field or an integer or long field with doc values. A multi-valued field is 400 (stock fails on it too).
- Refused as stock does: an unmapped field ("no mapping found for `f` in order to collapse on"), a field of another type ("unknown type for collapse field `f`, only keywords and numbers are accepted"), a field without doc values ("cannot collapse on field `f` without `doc_values`"), `rescore` with it ("cannot use `collapse` in conjunction with `rescore`"), an empty field ("field name is null or empty"). Refused as not served: `inner_hits`, `search_after`, an index of several shards (the merge across shards would have to drop repeated values), and a date field.
- No new setting: only a new accepted shape, with no measured downside.

## Acceptance criteria

- AC-1: collapsing on a keyword field answers one hit per value, the best of each, in the search's order, and `hits.total` counts every match.
- AC-2: `from` and `size` page the groups, not the documents.
- AC-3: a field sort orders the groups by their best hit under that sort, and the hit's `sort` and `fields` carry values.
- AC-4: collapsing on an integer field works the same, and a document without a value is one group.
- AC-5: each stock refusal above answers 400 with its message.
- AC-6: the candidate window grows by four each pass and stops at the cap, past which the search is refused rather than answered from a partial window.
- AC-7: a collapse adds no object-store request on a warm shard.

## Tests

Unit: `crates/reader/src/dsl/collapse_tests.rs`. Integration: `crates/node/tests/it/collapse.rs`.

## S3 request budget

None added: doc values are read as for any scored search.
