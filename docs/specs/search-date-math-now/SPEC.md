# `now` in a range, from the instant a search arrived

## Serves

The "date math in a range" refusal of [compatibility](../../compatibility.md). The reader already evaluates date math over a date field's default format (`now`, `anchor||`, `+1d`, `-2M`, `/d`) at the instant the query context carries, but a search that arrived over HTTP carried none, so a range with `"lte":"now"` or `"gte":"now-15m"` (every dashboard's time filter) answered 400 "date math in [now] is not supported by this node yet".

## Change

- A search takes the instant `now` means once, where it arrives on this node: `_search` (one index, several, a point in time), each `_msearch` item, `_search/template` and each `_msearch/template` item. Every shard and every item reads that one instant, as OpenSearch fixes `nowInMillis` per search request.
- A search that already carries an instant (the forwarded search of a Java coordinator) keeps it: the node's clock is never read for it.
- The instant is the node's wall clock in epoch milliseconds, in UTC.
- Not here: `_count`, `_validate/query`, `_rank_eval` and the by-query APIs, which still answer the refusal for a `now`; a range's `format` and `time_zone`, which [search-range-format-zone](../search-range-format-zone/SPEC.md) reads, and `relation`, which stays refused.

## Acceptance criteria

- AC-1: over one index, `lte: now` and `gte: now-1000y` match every document, `gt: now` and `gte: now+1d/d` match none, and a bound of `now+1d` with a rounded lower bound answers as written.
- AC-2: the same reads of `now` hold over a list of indexes, in an `_msearch` item and in a search template.

## S3 request budget

None added: only an instant is read from the clock.

## Switch

None: a refusal becomes an answer, and a search without `now` is unchanged.

## Tests

`crates/node/tests/it/search_now.rs` through HTTP, over the compat index whose `published` field is a date.
