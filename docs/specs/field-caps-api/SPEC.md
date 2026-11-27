# `_field_caps`

## Serves

Parity plan R10: the stock search-adjacent REST APIs. `GET|POST /{index}/_field_caps?fields=...` answers which fields the named indices map, with their type, searchability and aggregatability, as stock OpenSearch's `RestFieldCapabilitiesAction` does for the parameters in scope.

## Change

`oss-node` (feature `reader`) gains `search_field_caps`: the two routes of `/{index}/_field_caps`, registered by one line in `search::routes`, and `SearchService::handle_field_caps`. Each named index is opened as the search opens it (shard 0; every shard of an index has the one mapping) and its mapping's leaf fields, multi-fields included, are read from the open shard. The answer is `{"indices":[...],"fields":{name:{type:{type,metadata_field,searchable,aggregatable[,indices][,non_searchable_indices][,non_aggregatable_indices]}}}}`, fields and types sorted by name.

Capabilities by type, from the Java mappers: `searchable` is the field's `index` parameter; `aggregatable` is its `doc_values` parameter, and for `text` its `fielddata` parameter (false by default).

A field of the same type in some of the indices lists no `indices`; a field of several types lists, under each type, the indices that have it. A field of one type searchable (or aggregatable) in some of the indices and not in others is `false` and lists the others as `non_searchable_indices` (`non_aggregatable_indices`).

Not here: `/_field_caps` and a pattern or `_all` as the index (they need the index list; an alias of one index is served by [search-on-alias](../search-on-alias/SPEC.md)), the `fields` body key, `ignore_unavailable`, `include_unmapped`, `filters`, `types` and `index_filter`, and the metadata fields (`_id`, `_index`, `_source` and the rest), which this answer does not list.

## Acceptance criteria

- AC-1: `fields=*` over one index lists every mapped field, multi-fields included, with `type`, `metadata_field: false`, `searchable` and `aggregatable` by type, and `indices` naming the index.
- AC-2: `fields` takes a comma-separated list of names and `*` patterns; a name that matches nothing is left out and no match at all answers `"fields":{}`.
- AC-3: `index: false` makes a field not searchable, `doc_values: false` not aggregatable, and `fielddata: true` makes a text field aggregatable.
- AC-4: over several indices, a field of one type in all of them lists no `indices`, a field of different types lists each type with its `indices`, and a field not searchable in one of them lists `non_searchable_indices`.
- AC-5: no `fields` parameter answers 400 `action_request_validation_exception` ("no fields specified"); an unknown URL parameter answers the router's 400.
- AC-6: an unknown index answers the 404 `index_not_found_exception` a search does; `_field_caps` without an index, a wildcard or `_all` answers 400 `illegal_argument_exception` "not supported by this node yet".
- AC-7: over an index of two shards the answer is the one-shard answer.
- AC-8: `GET` and `POST` answer alike, and a call over a warm open index makes no object-store request.

## Test plan

`crates/node/tests/it/field_caps.rs` over the compat blobs (`compat-term`, and variants of its descriptor with other mappings as `put_variant` builds them), through HTTP. AC-8 reads the meter, as `it/count.rs` does.

## S3 request budget

The search's: a cold open of each named index is the descriptor, the commit and its ranged read; a warm open index costs nothing. LIST 0. Requests scale with the indices named, which the caller bounds.

## Switch

None: a new endpoint, refusing what it does not support.

## Risks

- The metadata fields are not listed, so `fields=*` answers fewer entries than OpenSearch does; listed under compatibility.
- An index of several mapping versions across shards is read from shard 0, as every shard shares the descriptor's mapping.
