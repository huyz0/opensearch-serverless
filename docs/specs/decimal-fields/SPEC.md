# Reader: `float` and `double` fields

## Serves

The reader's single-shard `_search` over `float` and `double` fields (principles: OpenSearch-compatible for the serverless scope). A detector whose metric field is a double, and a scripted metric over one, were refused as "not supported by this node yet".

## Change

The writer (`oss-doc`) and the engine (lucene-aggs, `ValueKind::Double` and `Float`) already index and read both types: `DoublePoint`/`FloatPoint` and sorted numeric doc values of the sortable long or int. The reader mapped them as `Unsupported`. This work gives the reader `FieldType::Float` and `FieldType::Double` and serves them in slices, one PR each:

1. Metric aggregations: `min`, `max`, `sum`, `avg`, `stats`, `value_count`, `cardinality`, `percentiles`, `percentile_ranks` at the top level and as sub-aggregations. A float widens to a double per value, as `SortedNumericDoubleValues` does.
2. `term`, `terms`, `match` and `range` queries, `sort` and `search_after`.
3. `docvalue_fields` and the `fields` API.
4. Bucket aggregations (`terms`, `histogram`, `range`, `composite`) with double keys.
5. `half_float` and `scaled_float`, if the engine's kinds read them as they are.

Until its slice, every other use of the two types is refused with the same "is not supported by this node yet" message `Unsupported` gave (`FieldType::refusal_name`). Slice 1 changes no other behaviour: a long, integer, date or keyword field is read as before.

## Acceptance criteria

- AC-1: `min`, `max`, `sum`, `stats` and `value_count` of a `double` field over 200 documents in four segments equal the values computed from the documents, the sum compensated in document order.
- AC-2: the same metrics of a `float` field read each value as the float's own double (a stored `19.9f` aggregates as `19.899999618530273`), not as the decimal written.
- AC-3: a metric over a `double` field aggregates a query's matches only, and `percentiles` of one is served.
- AC-4: a bucket aggregation (`terms`) over a `double` field is refused 400 "[terms] of the double field [d] is not supported by this node yet", until slice 4.
- AC-11: `docvalue_fields` of a `double` field writes the double and of a `float` field the float widened to a double (`0.10000000149011612` for the float nearest 0.1), as `SortedNumericDoubleValues` reads them.
- AC-12: the `fields` API reads a `double` from `_source` as a double and a `float` as a float, an empty string as no value, an infinity or non-number failing the write, and a `format` refused ("doesn't support formats").
- AC-5: a field type this reader still does not serve (`ip`) keeps its refusal, and a `long`, `integer` or `date` field is read unchanged (the existing metric tests pass).
- AC-6: `term`, `match` and `terms` on a `double` field match the documents holding the value, and on a `float` field the documents holding the float nearest the value (`0.1` finds the stored `0.1f`).
- AC-7: `range` on a `double` or `float` field honours each bound, an excluded bound stepping one double (float) in as `DoublePoint.nextUp` and `nextDown` do, `-0.0` and `+0.0` included; an empty interval matches nothing.
- AC-8: a value that is not a number, or not finite, is refused as `NumberType.parse` refuses it ("For input string", "[double] supports only finite values, but got [Infinity]").
- AC-9: a `sort` on a `double` field orders across segments and reads back as a Java double, on a `float` field as a Java float; missing values sort last.
- AC-10: `search_after` resumes a `double` and a `float` sort, and a text that is not a number fails as Java's `Double.parseDouble` does.

## Test plan

`aggs/metric/collect_decimal_tests.rs` over an `oss-testkit` index with `Spec::with_decimals` (a `double` `d` and a `float` `f`): AC-1 to AC-5; `search_decimal_tests.rs` and `numbers_decimal_tests.rs` hold AC-6 to AC-10 over the same index; each of them failed before the change with "aggregating a [double] field is not supported by this node yet".

Expected values are computed from the generator, not recorded from Java 3.10.0: the engine's kinds port `SortedNumericDoubleValues`. A recorded step against stock OpenSearch joins a later slice, once one runs here.

## S3 request budget

Unchanged: doc values are read through the same page cache and ranged GETs as a long's. LIST 0, HEAD 0, PUT 0; an idle index makes none. The `search.metric_aggs.*` budget rows read a long field and bound the path.

## Switch

None: a request over these fields was refused before; every other request is unchanged.

## Risks

A float's widening or a double's sum order differing from Java shows as a mismatched recording once one exists. The reader's `FieldType` gains two variants, so a later use of the type that matches without a wildcard fails to compile rather than serving a long's behaviour by mistake.
