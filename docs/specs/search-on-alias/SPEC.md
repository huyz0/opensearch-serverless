# `_search`, `_count` and `_msearch` on an alias of one index

## Serves

The "alias in a search path" gap of [compatibility](../../compatibility.md): the alias routes ([alias-routes](../alias-routes/SPEC.md)) make and read aliases, but a search naming one answers `index_not_found_exception`, because the reader opens only concrete indexes. This serves the common case, an alias of exactly one index (a write alias, a rollover alias, a data stream's alias). An alias of several indexes is served by `_search` ([search-several-indexes](../search-several-indexes/SPEC.md)) and stays refused by `_count`, `_msearch` and `_field_caps`.

## Change

- `SearchService` gains an alias source, installed like the pipeline and template sources: a trait that answers the members of a name, backed on a node by the alias registry (`DescriptorStore::resolve`, one register read for an alias). None installed, a name that is no index is `index_not_found_exception` as today.
- A search, count or `_msearch` item naming a name that is not an open concrete index asks the source once. An alias of one index runs against that index: the hits' `_index`, the response and every later step use the concrete name, as OpenSearch reports. An alias of several indexes answers 501 `unsupported_alias_option` on `_count`, `_msearch` and `_field_caps`. A name that is neither is 404 `index_not_found_exception` naming the name asked.
- A name that is an open concrete index is never looked up as an alias: a warm search of an index makes no new request. A cold open reads what it read before.
- Aliases are not cached: each request naming an alias reads its register again, so an alias change is seen at once.
- `_field_caps` takes the same step for each name of its index list: an alias of one index is that index, and the answer's `indices` names it.
- Not here: `routing` and `preference`, an alias in a point-in-time search, alias filters (the routes refuse them), and a data stream's several backing indexes.

## Acceptance criteria

- AC-1: `POST /{alias}/_search` of an alias of one index answers what `/{index}/_search` answers, with `_index` the concrete name in every hit.
- AC-2: `_count` and an `_msearch` item naming the alias count and search the same way.
- AC-3: after the alias is removed from the index, the next request answers 404 `index_not_found_exception`; after it is added to another index, the next request searches that one.
- AC-4: an alias of two indexes answers 501 `unsupported_alias_option` on `_count` (`_search` serves it, see search-several-indexes).
- AC-5: a warm search of a concrete index makes no object-store request more than before; a search of an alias costs one GET for the failed open of its own name and the alias register's one GET, on top of the index's.
- AC-6: a name that is neither an index nor an alias is 404 as today, with no alias read when no source is installed.
- AC-7: `GET /{alias}/_field_caps?fields=*` of an alias of one index answers what the index answers, `indices` naming the index; an alias of several is 501.

## S3 request budget

An alias search adds two GETs to the search it resolves to (the failed open of the alias's own name, and the alias register); no LIST, nothing scaling with members or time. A concrete index adds none.

## Switch

None: it turns a 404 into an answer, and adds no request to a name that already worked.

## Tests

`crates/node/tests/it/search_alias.rs` over a node with the alias routes, through HTTP; AC-5 reads the store meter.
