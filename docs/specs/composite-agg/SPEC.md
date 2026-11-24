# Reader: the `composite` aggregation

## Serves

Roadmap R10.2: OpenSearch's `composite` of a single-shard `_search` at the top level, with a shard-level result a coordinator reduces across shards (I-13, REST matches OpenSearch; ADR 0004, bounded memory). The engine side (`AggNode::Composite`) is in lucene-aggs already.

## Change

A top-level `composite` with sources of type `terms` (a keyword, long, integer or date field; a date's key is its milliseconds as a long, as OpenSearch writes it), `histogram` (a numeric field) and `date_histogram` (a date or long field, with `calendar_interval` or `fixed_interval`, a fixed-offset `time_zone` and `offset`; its key is the rounded milliseconds as a long), each with `order`, `missing_bucket` and `missing_order`, and the aggregation's `size` (default 10) and `after`. `Value::Composite` holds the shard's keys and counts; its reduce merges the shards' buckets in key order and cuts at `size`.

Not here, each refused with 400 "... is not supported by this node yet": a `date_histogram` source's `format`, `interval` and region time zone, a source's `script` and `value_type`, a bucket aggregation, `top_hits` or `percentiles` under a `composite` (only the scalar `min`, `max`, `sum`, `avg`, `stats` and `value_count` are served beneath it), a `composite` under another aggregation, `size` below 1. A `size` over `search.max_buckets` fails with 503 `too_many_buckets_exception`, as a reduce that makes that many buckets does (AC-6).

## Acceptance criteria

- AC-1: a `composite` of one `terms` source on a keyword, a long and a date field lists the buckets in key order (ascending, or descending with `order: desc`), each with its document count, at most `size`, and writes `after_key` as the last bucket's key (no `after_key` without a bucket).
- AC-2: with several sources the key is every source's value and buckets sort by the first source, then the next.
- AC-3: `after` resumes past the key given: paging with `after` visits every bucket once and in the order of an unpaged search; an `after` with a missing source or the wrong count of values fails with `illegal_argument_exception` as OpenSearch words it.
- AC-4: `missing_bucket: true` gives documents without a value a `null` key that sorts first ascending and last descending, and `missing_order` `first` or `last` fixes where; `missing_order` without `missing_bucket` is refused as OpenSearch refuses it.
- AC-5: a `histogram` source keys a document by `floor(v / interval) * interval` as a double; an `interval` of 0 or less is refused as OpenSearch refuses it.
- AC-6: the reduce of shard results merges equal keys by adding their counts, orders by the sources' order and missing rules, cuts at `size`, and counts the buckets it makes against `search.max_buckets`.
- AC-7: the body is parsed as `CompositeAggregationBuilder` parses it: no or empty `sources`, duplicate source names, unknown keys and an unknown source type fail with OpenSearch's messages; the refusals above are 400s naming what is refused.
- AC-8: a search with a `composite` reserves for its kept keys (`size` times the sources) and for each keyword source's ordinals with the hits.
- AC-9: a cold search with a `composite` costs the rows `search.composite.*` of [request-budgets.md](../../request-budgets.md): the cold requests and round trips of the same search without it, and no request when warm.

- AC-10: a `date_histogram` source keys a value by its interval's start (milliseconds, a long), pages with `after` and orders like a `terms` source; it needs exactly one valid interval, refused with OpenSearch's messages, and a keyword field fails the shard.
- AC-11: a `composite` runs scalar metric sub-aggregations (`min`, `max`, `sum`, `avg`, `stats`, `value_count`) per bucket, reduced over the shards that hold the bucket and written after `doc_count`; a bucket aggregation under it is refused by name, and their state is reserved with the keys.

## Test plan

Unit tests beside the code (`aggs/bucket/composite_parse_tests.rs`, `composite_tests.rs`, `composite_collect_tests.rs`): the parser and the reduce and writer on constructed values, the collection on the real four-segment index of the keyword tests (`tag`, `n`). The request budget rows are `search.composite.*`, spent by `crates/node/tests/it/aggs_budgets.rs`.

## S3 request budget

Unchanged: the same doc-value and ordinal reads as a `terms` over the same fields. LIST 0, HEAD 0, PUT 0.

## Switch

None: the request was refused before, and a request without a `composite` is unchanged.

## Risks

No stock 3.10.0 recording backs these criteria; expected bodies are read from the Java (`InternalComposite`, `CompositeAggregation`, `CompositeAggregationBuilder`). The engine's `sorted_lead` (a first source read through points or terms, which Java picks per query) is passed false, so a document with several values of the first source is counted as the document pass counts it. No multi-valued field is in the test index, so that case is untested here. `nested` and `reverse_nested` stay refused: this node maps no `nested` field, so there are no child documents to join.
