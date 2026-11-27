# `rest_total_hits_as_int` on `_search`

## Serves

The `rest_total_hits_as_int` gap of [compatibility](../../compatibility.md): the legacy clients and Kibana-era tools send it, and a `_search` naming it answers 400 (unknown parameter).

## Change

- `?rest_total_hits_as_int=true` writes the response's `hits.total` as a number (`"total":5`), not `{"value":5,"relation":"eq"}`, on a search of one index, of several, and of none (`ignore_unavailable`).
- As in OpenSearch it counts exactly: a body or URL `track_total_hits` of a number or `false` answers 400 "[rest_total_hits_as_int] cannot be used if the tracking of total hits is not accurate, got N" (`-1` for `false`); the default cap becomes exact.
- `false` is the same as the parameter absent; any other value answers the boolean-parse 400 of [search-url-params](../search-url-params/SPEC.md).
- Differs: the total inside a `top_hits` aggregation and inside `inner_hits` stays the object.

## Acceptance criteria

- AC-1: on one index, the total is a number and the other keys are unchanged.
- AC-2: `track_total_hits=3` and `track_total_hits=false` beside it answer 400 with the message above; `track_total_hits=true` is accepted.
- AC-3: over a list of several indexes the total is a number.
- AC-4: with `ignore_unavailable` and no index available the total is `0`.

## S3 request budget

None added: only how the total is written changes.

## Switch

None: it turns a refusal into an answer.

## Tests

`crates/node/tests/it/search_url_params.rs` through HTTP, and the `url_params` unit tests of the reader.
