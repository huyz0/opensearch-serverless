# Reader: `multi_terms` aggregation over keyword and long fields

## Serves

Roadmap R10.2: OpenSearch's `multi_terms` bucket aggregation over two or more keyword, integer or long fields of a single-shard `_search`, with a shard-level result a coordinator reduces across shards (I-13, REST matches OpenSearch; ADR 0004, bounded memory).

## Change

`oss-reader` parses `multi_terms` (`MultiTermsAggregationBuilder` at OpenSearch 3.10.0): `terms` (an array of at least two objects, each with a `field`), `size` (10), `shard_size` (`size * 1.5 + 10`, never below `size`), `min_doc_count` (1), `shard_min_doc_count` (0, capped at `min_doc_count`) and `order` (`_count` or `_key`, `asc` or `desc`, one criterion). It runs the engine's `composite` node over the terms (the engine counts every combination of their values in one pass, a document without a value for a term in none), and keeps the best `shard_size` combinations by the order. The shard result carries the buckets, the documents of the others (`sum_other_doc_count`) and the error bound; the reduce is `InternalTerms.reduce`: equal keys add, the buckets under `min_doc_count` go, the rest are ordered and cut at `size`, and the error bound is the sum of the shards' (a full shard's is its smallest count). A key orders term by term: a keyword by its bytes, a number by its value.

The response is `doc_count_error_upper_bound`, `sum_other_doc_count` and `buckets`, each `{"key":[<term>...],"key_as_string":"<term>|<term>","doc_count":n}`; `typed_keys` names it `multi_terms`.

Not in this change, each refused as "not supported by this node yet": a term's `missing`, `value_type`, `format`, `script`, `time_zone`, `include` and `exclude`; `collect_mode`; `show_term_doc_count_error`; an `order` by a sub-aggregation or of several criteria; a date, float or double term; sub-aggregations; and a `multi_terms` under another aggregation. A shard with more combinations than the bucket limit (65,535) fails with `too_many_buckets_exception`, where OpenSearch's collector would carry on.

## Acceptance criteria

- AC-1: the body parses with its defaults; fewer than two terms fail with "multi term aggregation must has at least 2 terms. Found [n] in [name]", a `size` or `shard_size` of 0 or less with "[size] must be greater than 0. Found [n] in [name]" ("[shardSize]" for the latter), a negative `min_doc_count` or `shard_min_doc_count` with "[minDocCount]" or "[shardMinDocCount] must be greater than or equal to 0. Found [n] in [name]", a body with no `terms` with "[terms] must not be null. Found null terms in [name]", and an unknown key as the other aggregations' do.
- AC-2: the response body is the bytes OpenSearch writes after the reduce (`fixtures/multi-terms/java-multi-terms-3.10.0.txt`, written by `MultiTermsRecordings.java` from the 3.10.0 jars): the keys as lists and as `a|1`, the sum of other documents, and the error bound, for one shard that cut buckets, two shards that filled their `shard_size`, and equal keys across shards.
- AC-3: equal counts order by key, term by term, a keyword by its bytes and a number by value (`ties_text`, `ties_number`, `ties_mixed` of the fixture).
- AC-4: over an index, a shard's buckets are the combinations of the terms' values of the matches, counted once a document; a document without a value for a term is in none; a term on an unmapped field answers no bucket.
- AC-5: a shard keeps `shard_size` buckets by the order, and the documents of the others, and of those under `shard_min_doc_count`, are in `sum_other_doc_count`.
- AC-6: every unserved option above is refused by name, and a date term is refused when the shard runs.

## Test plan

`aggs/bucket/multi_tests.rs`: the parse (AC-1, AC-6) and the recorded bodies (AC-2, AC-3) as unit tests, and a shard over an `oss-testkit` index (AC-4, AC-5, AC-6). The recorder compiles against the OpenSearch 3.10.0 jars as `fixtures/search-shard/CardRecordings.java` does. The shard's own result (the buckets a shard keeps, its `other_doc_count`) is read from `MultiTermsAggregator` and `InternalTerms.reduce`, not recorded: no stock 3.10 build runs here.

## S3 request budget

The pass reads each term's doc values and dictionaries as a `composite` does. LIST 0, HEAD 0, PUT 0; an idle index makes none.

## Switch

None: a `multi_terms` was refused before, and a request without one is unchanged.

## Risks

`show_term_doc_count_error` is refused because the recorded per-bucket error of a key held by several shards (`2` for two shards with errors 1 and 2, where the shared reduce arithmetic of this crate's `terms` gives 0) is not understood yet; the total `doc_count_error_upper_bound` matches. A shard's default `shard_size` for a key order, and the multi-valued field's combinations, are read from the Java and not observed.
