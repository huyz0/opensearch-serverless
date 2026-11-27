# Named queries: `_name` and `matched_queries`

## Serves

The `_name` refusal of [compatibility](../../compatibility.md): a query that carries `_name` answers 400 today. Clients name the clauses of a `bool` to learn which of them matched each hit.

## Change

Ported from `AbstractQueryBuilder.queryName`, `QueryShardContext.addNamedQuery`, `MatchedQueriesPhase` (`FetchSubPhase`) and `SearchHit.toInnerXContent` at OpenSearch 3.10.0.

- `match_all`, `term`, `terms`, `match`, `range`, `bool`, `prefix`, `wildcard`, `regexp`, `fuzzy`, `exists` and `ids` take `_name`; every other query keeps refusing it as not supported yet, and is the next slice (a string; an empty string is a name, null is none, as Java). A name read before the query's own clauses stays the query's. The parser carries the name out of the query's own parse and wraps the query; building the Lucene query ignores the wrapper.
- A search whose query tree holds named queries writes `matched_queries` on each hit: the names of the named queries that match that hit's document, in the order the names appear in the request (OpenSearch lists them in the order of a hash map, which a client cannot rely on). With `include_named_queries_score: true` it is an object of name to that query's score for the hit. A name used twice is listed once.
- A query is checked against a hit by explaining the named query alone on the hit's document (the engine's `explain`), at fetch time over the page of hits only.
- At most 64 named queries a search; a request with more is 400 "too many named queries [n]; the limit is 64".
- A search with no named query makes no extra work and writes the hit as before.
- Not here: names inside `post_filter`, `rescore`, aggregation filters and inner hits (refused as today); `_name` on queries inside `nested` is read and matched on the parent hit only when the nested query itself is named.

## Acceptance criteria

- AC-1: the query kinds above parse `_name`; a non-string value is 400 as Java ("[_name] ... expected a string").
- AC-2: `bool` with named `must`, `should` and `filter` clauses writes the names of those that match each hit, and not those that do not.
- AC-3: a named clause under `must_not` is never listed (its hits do not match it).
- AC-4: `include_named_queries_score` writes an object of name to score.
- AC-5: more than 64 named queries is 400.
- AC-6: a search with no named query makes the same object-store requests as before, and pays nothing per hit.
- AC-7: naming a query does not change which hits match, their scores or their order.

## S3 request budget

None added: the check reads the documents of the hit page the fetch already holds open.

## Switch

None: refusals become answers, and a search without `_name` writes as before.

## Tests

`crates/reader/src/dsl/named_tests.rs` for the parse, `crates/node/tests/it/search_named_queries.rs` through HTTP. No recording from stock OpenSearch exists, so expectations are derived from the Java named above and are provisional until recorded.
