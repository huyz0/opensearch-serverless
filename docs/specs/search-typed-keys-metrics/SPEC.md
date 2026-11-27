# `typed_keys` on `_search`, metric aggregations

## Serves

The `typed_keys` gap of [compatibility](../../compatibility.md): the official clients send `typed_keys=true` on every search with aggregations, and a `_search` naming it answers 400 (unknown parameter).

## Change

- `?typed_keys=true` writes each top-level aggregation's key as `type#name`, with the type OpenSearch's `getWriteableName` gives: `min`, `max`, `sum`, `avg`, `stats`, `value_count`, `cardinality`, `tdigest_percentiles`, `tdigest_percentile_ranks` and `top_hits`. Values and the order of the aggregations are the untyped answer's, which orders them by the plain name as OpenSearch's map does.
- It holds over one index and over several (the coordinator writes the merged result with the same writer). `typed_keys=false`, or none, changes nothing; a search without aggregations answers as without the parameter.
- A bucket aggregation (`terms`, `histogram`, `date_histogram`, `range`, `filters`, `filter`, `composite`, `nested`, `reverse_nested`), whose sub-aggregations are typed too, answers 400 "the [typed_keys] with a [terms] aggregation is not supported by this node yet". Each family becomes served in its own change.
- Not here: `typed_keys` on `_msearch`, which stays refused, and on `_count`, which has no aggregations.

## Acceptance criteria

- AC-1: each metric above answers `"<type>#<name>"` and the same values in the same order as without the parameter.
- AC-2: over a list of indexes the merged aggregations are typed.
- AC-3: with no aggregations, or `typed_keys=false`, the answer is the plain one; a bucket aggregation answers 400 naming it; a non-boolean value answers the boolean-parse 400.

## S3 request budget

None added: only the key written changes.

## Switch

None: it turns a refusal into an answer.

## Tests

`crates/node/tests/it/search_url_params.rs` through HTTP, and the `url_params` unit tests of the reader.
