# Reader: `float` and `double` fields

## Serves

The reader's single-shard `_search` over `float` and `double` fields (principles: OpenSearch-compatible for the serverless scope). A detector whose metric field is a double, and a scripted metric over one, were refused as "not supported by this node yet".

## Change

The writer (`oss-doc`) and the engine (lucene-aggs, `ValueKind::Double` and `Float`) already index and read both types: `DoublePoint`/`FloatPoint` and sorted numeric doc values of the sortable long or int. The reader mapped them as `Unsupported`. This work gives the reader `FieldType::Float` and `FieldType::Double` and serves them in slices, one PR each:

1. Metric aggregations: `min`, `max`, `sum`, `avg`, `stats`, `value_count`, `cardinality`, `percentiles`, `percentile_ranks` at the top level and as sub-aggregations. A float widens to a double per value, as `SortedNumericDoubleValues` does.
2. `term`, `terms`, `range` and `match` queries, `sort`, `docvalue_fields` and the `fields` API.
3. Bucket aggregations (`terms`, `histogram`, `range`, `composite`) with double keys.
4. `half_float` and `scaled_float`, if the engine's kinds read them as they are.

Until its slice, every other use of the two types is refused with the same "is not supported by this node yet" message `Unsupported` gave (`FieldType::refusal_name`). Slice 1 changes no other behaviour: a long, integer, date or keyword field is read as before.

## Acceptance criteria

- AC-1: `min`, `max`, `sum`, `stats` and `value_count` of a `double` field over 200 documents in four segments equal the values computed from the documents, the sum compensated in document order.
- AC-2: the same metrics of a `float` field read each value as the float's own double (a stored `19.9f` aggregates as `19.899999618530273`), not as the decimal written.
- AC-3: a metric over a `double` field aggregates a query's matches only, and `percentiles` of one is served.
- AC-4: a bucket aggregation (`terms`) over a `double` field is refused 400 "[terms] of the double field [d] is not supported by this node yet", until slice 3.
- AC-5: a field type this reader still does not serve (`ip`) keeps its refusal, and a `long`, `integer` or `date` field is read unchanged (the existing metric tests pass).

## Test plan

`aggs/metric/collect_tests.rs` over an `oss-testkit` index with `Spec::with_decimals` (a `double` `d` and a `float` `f`): AC-1 to AC-5; each of them failed before the change with "aggregating a [double] field is not supported by this node yet".

Expected values are computed from the generator, not recorded from Java 3.10.0: the engine's kinds port `SortedNumericDoubleValues`. A recorded step against stock OpenSearch joins a later slice, once one runs here.

## S3 request budget

Unchanged: doc values are read through the same page cache and ranged GETs as a long's. LIST 0, HEAD 0, PUT 0; an idle index makes none. The `search.metric_aggs.*` budget rows read a long field and bound the path.

## Switch

None: a request over these fields was refused before; every other request is unchanged.

## Risks

A float's widening or a double's sum order differing from Java shows as a mismatched recording once one exists. The reader's `FieldType` gains two variants, so a later use of the type that matches without a wildcard fails to compile rather than serving a long's behaviour by mistake.
