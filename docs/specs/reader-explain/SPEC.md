# Reader: explain

## Serves

Parity task R10.4 (hit features), fourth part; invariant I-13 (REST matches OpenSearch's recordings); ADR 0004 (bounded memory).

## Change

`oss-reader` serves the search body's `explain`: each hit carries the explanation of its score. It is ported at OpenSearch 3.10.0 from `ExplainPhase` and `SearchHit.toInnerXContent` (`crates/reader/src/fetch/explain.rs`, wired in `dsl/source.rs`, `search.rs` and `response.rs`).

- `explain` is a boolean or a string of one; `false` is the default.
- A hit then starts with `"_shard":"[index][0]","_node":"<node id>"` before `_index`, and ends with `"_explanation":{"value","description","details"}` after `sort`.
- The explanation is the engine's for the query a search scored, the hits' documents each explained against the whole shard's statistics. Two names differ in the engine and are renamed where it writes them: the similarity (`[PerFieldSimilarity]`, as OpenSearch scores per field), and a constant score over a keyword term, which the engine counts twice.
- A `dis_max` with a tie breaker of 1 (and so `multi_match` `most_fields`) explains as `sum of:`, as Lucene's rewrite makes it; a filter clause names its query alone.
- A search with no hit has nothing to explain and answers as without `explain`.
- `write_response` takes the node id, which the node already holds, for `_node`.

Not in this change: `explain` of a query the engine does not explain as OpenSearch does is refused as not supported yet: a `range` or any points query (OpenSearch names it `Approximate(...)`), `exists`, `ids` (OpenSearch prints the encoded ids as bytes) and a `constant_score` with a `boost`.

The sort code of `search.rs` moved into `search_sort.rs` to keep both modules small; it is unchanged.

## Acceptance criteria

- AC-1: every `_search` step of `fixtures/compat/search/match.json` gets the status and body OpenSearch 3.10.0 gave, replayed against a reader node over the same blobs. That is the existing steps plus 26 new ones: a match, two words, `operator: and`, a boost, a keyword, `match_phrase`, `multi_match` of both kinds, `match_all`, `term`, a bool with a filter, with `must_not` and with `should`, `prefix`, `wildcard`, `terms`, a plain `constant_score`, `dis_max`, a sort with `docvalue_fields`, two hits, a `_source` filter, no hit, `explain: false`, `explain: "true"` and `explain: null`.
- AC-2: `_shard`, `_node` and `_explanation` are written around each hit in OpenSearch's order, and by no hit of a search without `explain`.
- AC-3: the refused queries are refused, wherever they sit in a bool, `dis_max` or `constant_score`, with one message.
- AC-4: a tie breaker of 1 explains as a sum, a filter and a constant score over a keyword term are named as OpenSearch names them, and the similarity is `PerFieldSimilarity`.

## Test plan

- AC-1: the replay (`crates/compat/tests/node_search.rs`).
- AC-2 to AC-4: `fetch/explain_tests.rs`.

## S3 request budget

A search without `explain` is unchanged. With it, each hit is explained from the term statistics and norms the query already read, plus its own postings: the reads scale with the bytes of the hits' postings pages, never with documents of the index. LIST 0, HEAD 0, PUT 0. Idle: 0.

## Memory

An explanation is built for each hit of the result window at most (10,000 hits), each as large as its query's tree; the response writes them one hit at a time. They are counted with the hits' bytes by `hits_bytes`' per-hit size only as the `Explanation` handle, so a very large `size` with a very large query holds more than counted. The window bounds hits and the query's recording bounds its tree.
