# `_search` over several indexes

## Serves

The "alias of several indexes" gap of [search-on-alias](../search-on-alias/SPEC.md), and the comma list of index names, which both answered an error. The coordinator already merges the answers of many shards and takes each shard's index from the shard, so a search over the shards of several indexes is the same merge over a longer list.

## Change

- `POST|GET /{a},{b}/_search` over concrete index or alias names, and `/{alias}/_search` of an alias of several indexes, query every shard of every index the names resolve to (duplicates once) and merge the answers as one response: the `total` and `_shards` count them all, and every hit's `_index` is its own index. Sorts and aggregations go through the same merge the shards of one index do; this change adds no test for either over two indexes.
- Each name is resolved as in [search-on-alias](../search-on-alias/SPEC.md): an index that opens is never looked up as an alias, an alias costs one register read. A name that is neither is 404 `index_not_found_exception` naming it, and the search answers nothing.
- `routing` narrows each index to the shards its values route to ([search-routing](../search-routing/SPEC.md)).
- The search runs no index's `index.search.default_pipeline`: OpenSearch applies a default pipeline only when the request resolves to one index. An explicit `search_pipeline` parameter or body field runs as for one index.
- The search takes the admission permit of its first name, as a search of one index does ([search-slots](../search-slots/SPEC.md)). A search may name or resolve to at most 64 indexes and visit at most 1,024 shards; past either it is a 400 before any shard is read.
- Nothing here lists anything: a name is a literal name. A pattern or `_all` stays as it was (a 404 for an index that does not exist). No default pipeline of a single-index list is exercised by a test here.
- Not here: `_count`, `_msearch`, `_field_caps` and the other APIs over several indexes (an alias of several indexes is still 501 there), `collapse` (400, as over several shards), a point-in-time search (400), and `ignore_unavailable` / `allow_no_indices`.

## Acceptance criteria

- AC-1: `/{a},{b}/_search` of a `match_all` has a `total` that is the sum of the two indexes' totals, `_shards.total` the sum of their shards, and hits from both, each with its own `_index`.
- AC-2: `/{alias}/_search` of an alias of two indexes answers what `/{a},{b}/_search` answers, and an alias with an index of its own also in the list searches that index once.
- AC-3: a list that names an index that does not exist is 404 `index_not_found_exception` naming it.
- AC-4: `size` merges across the indexes: a page of 3 over two indexes has 3 hits, and the `total` is still the whole.
- AC-5: `collapse` and a point in time with several indexes are 400 refusals.
- AC-6: a list search holds the permit of its first name for its length and gives it back.
- AC-7: a search naming more than 64 indexes is a 400 naming the limit.
- AC-8: `routing` narrows each index of the list to the shards its values route to.
- AC-9: a cold search of a list makes exactly the requests of the cold searches of its indexes added, and no LIST.
- AC-10: an alias with a member that is no index answers 404 naming that member alone.
- AC-11: a list of one name, and a name that is an alias of one index, answer exactly as before.

## S3 request budget

The requests of searching each index on its own (their descriptor and commit reads), and one register read for each name that is not an open index (the stub source in the test costs none, so AC-9 pins the index reads). Nothing scales with documents or time; no LIST.

## Switch

None: an error becomes an answer, and a single index costs what it did.

## Tests

`crates/node/tests/it/search_several.rs`, through HTTP over the compat fixture indexes and a stub alias source.
