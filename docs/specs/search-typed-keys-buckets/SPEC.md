# `typed_keys` on `_search`, bucket aggregations

## Serves

The second part of [search-typed-keys-metrics](../search-typed-keys-metrics/SPEC.md): with `typed_keys=true`, a search with a bucket aggregation answered 400 "[typed_keys] with a [terms] aggregation is not supported by this node yet".

## Change

- A bucket aggregation's key is written `type#name` with the type of its result class: `lterms` (a `terms` of an integer, long or date field), `sterms` (a keyword field), `histogram`, `date_histogram`, `range`, `filters`, `filter` and `composite`.
- The sub-aggregations of every bucket are typed in the same way, at every depth, metric or bucket (`max#m` inside `lterms#t`).
- It holds over one index and over several, since both write through the same writer, with the setting held for the one write in a thread-local scope so the bucket writers need no parameter.
- `nested` and `reverse_nested`, anywhere in the tree, still answer 400 "the [typed_keys] with a [nested] aggregation is not supported by this node yet": their result is held as a `filter`'s is, so its type name cannot be told from it.

## Acceptance criteria

- AC-1: one request holding each served bucket aggregation, with metric and bucket sub-aggregations, answers every key typed as above, and without `typed_keys` no key holds a `#`.
- AC-2: over a list of indexes a merged `terms` and its sub-aggregation are typed.
- AC-3: a `nested` aggregation answers 400 naming it; a `filter` is `filter#name`.

## S3 request budget

None added: only the keys written change.

## Switch

None: it turns a refusal into an answer.

## Tests

`crates/node/tests/it/search_url_params.rs` through HTTP, the `typed` unit tests of the reader and the `url_params` ones.
