# Reader: `rare_terms` aggregation over a keyword or number field

## Serves

Roadmap R10.2: OpenSearch's `rare_terms` bucket aggregation over a keyword, integer or long field of a single-shard `_search`, with a shard-level result a coordinator reduces across shards (I-13, REST matches OpenSearch; ADR 0004, bounded memory).

## Change

`oss-reader` parses `rare_terms` (`RareTermsAggregationBuilder` at OpenSearch 3.10.0): `field`, `max_doc_count` (default 1, from 1 to 100) and `precision` (default 0.001, above 0.00001; read and not used, because this node counts exactly). It runs the engine's `SignificantTerms` node (`terms: true` for a keyword, `numeric: true` for a number), which counts every term of the field with no cut, and keeps the terms held by at most `max_doc_count` matching documents (`StringRareTermsAggregator`). The shard result also carries the terms that passed `max_doc_count` (what OpenSearch's `SetBackedScalingCuckooFilter` remembers), so that the reduce drops a term that is rare on one shard and common on another (`InternalRareTerms.reduce`). The answer is `buckets`, each `key` and `doc_count`, by count then key ascending.

Not in this change, each refused as "not supported by this node yet": `include`, `exclude`, `missing`, `value_type`, `script`, a date field, sub-aggregations, and a `rare_terms` under another aggregation. A field with more distinct terms in the shard than the bucket limit (65,535) fails with `too_many_buckets_exception` where OpenSearch's cuckoo filter would carry on.

## Acceptance criteria

- AC-1: the body parses with its defaults (`max_doc_count` 1, `precision` 0.001); a `max_doc_count` of 0 or less fails with "[max_doc_count] must be greater than 0. Found [n] in [name]", one over 100 with "[max_doc_count] must be smallerthan 100in [name]" (OpenSearch's text), a `precision` below 0.00001 with "[precision] must be greater than 0.00001"; a missing `field`, an unknown key and every unserved key above fail as the other aggregations' do.
- AC-2: a shard's buckets are the terms of the matches held by at least one and at most `max_doc_count` matching documents, each counted once a document; the terms above it are in the result's excluded list.
- AC-3: the reduce adds each term's counts across shards, drops a term any shard excluded, keeps those with at most `max_doc_count`, and orders them by count ascending, then term ascending.
- AC-4: the response is `"buckets":[{"key":<term>,"doc_count":<n>}]`, `typed_keys` names it `srareterms`, and a keyword field the shard does not map answers no bucket.
- AC-5: over an index, the buckets are the rare terms of the query's matches, and a field with more distinct terms than the bucket limit fails with `too_many_buckets_exception` at 503.
- AC-6: over an integer or long field the terms are numbers (`LongRareTerms`): the answer's keys are JSON numbers, `typed_keys` names it `lrareterms`, and a date field is refused.

## Test plan

Unit tests in `aggs/bucket/rare_tests.rs` (AC-1 to AC-4) and over an `oss-testkit` index in `aggs/bucket/rare_collect_tests.rs` (AC-2, AC-5). Expected values come from reading `RareTermsAggregationBuilder`, `StringRareTermsAggregator` and `InternalRareTerms` at 3.10.0 (the messages from the jar's constants); no stock recording exists yet.

## S3 request budget

The pass reads the field's doc values and dictionaries as a keyword `terms` does. LIST 0, HEAD 0, PUT 0; an idle index makes none.

## Switch

None: a `rare_terms` was refused before, and a request without one is unchanged.

## Risks

No stock 3.10.0 recording backs these steps. OpenSearch answers exactly while its filter holds a set and approximately beyond it, where a false positive can hide a rare term; this node is exact up to the bucket limit and fails past it. The order of the answer's buckets is read from the Java, not observed.
