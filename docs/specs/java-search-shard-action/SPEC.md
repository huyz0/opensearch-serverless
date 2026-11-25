# A Rust owner answers the Java search-shard action

## Serves

Parity plan R15, the Java-retire gate. In a mixed fleet a Java coordinator whose reader is behind a Rust owner's acknowledged writes asks the owner to search the shard (`internal:serverless/search/shard`). Rust registers no handler, so the shard fails and the coordinator answers a partial result; the wide soak fails its first cycle ([ADR 0057](../../adr/0057-a-rust-owner-answers-the-java-search-shard-action.md)).

## Change

The Rust node registers `internal:serverless/search/shard`. It verifies the MAC over the received bytes, reads `ForwardedSearchRequest` (task header, index, vint shard, `SearchSourceBuilder`, optional uuid, long `now`) under limits, checks it writes the shard for that incarnation under a live lease, searches the shard's current view including unpublished acknowledged writes, checks again, and answers `ForwardedSearchResponse` (vlong total, hits, aggregations flag `false`, relation, max score, timed out, terminated early). Anything else is refused by name.

## Acceptance criteria

- AC-1: `ForwardedSearchRequest` bytes written by the Java shell (fixtures, with the Java build that wrote them recorded) for each supported query, including non-default `boost`, `_name`, `range` `format` and `time_zone`, and `bool` `minimum_should_match`, decode to the request the Rust search takes.
- AC-1b: a request with an unsupported query or field before later fields is refused naming the first unsupported item, and nothing after it is read.
- AC-2: a `ForwardedSearchResponse` written for the same hits is equal byte for byte to the Java writer's fixture, at stream version 3.10.0, including the `false` aggregations flag.
- AC-3: a forwarded search for a shard this node writes returns documents that are acknowledged and not yet published.
- AC-4: a forwarded search for a shard this node does not hold, for a uuid other than the held shard's, or after the node's lease is lost (including lost between the check before the query and the check after it), is refused as not owner and makes no blob request.
- AC-5: a request with a wrong MAC is refused with nothing decoded; a request over each limit by one (bytes, depth, clauses, `from` plus `size`, answer bytes) is refused, and one at each limit is served; a string or list length larger than the bytes left is refused before any allocation of that size; a negotiated version below 3.10.0 is refused.
- AC-6: a `range` on `now-1h` is evaluated at the request's `nowInMillis`: a document on that boundary is counted as the Java shard counts it.
- AC-7: hits that tie on score are ordered by `_id` ascending, checked with a fixture of tied scores.
- AC-8: a forwarded search takes a slot from the bounded pool; with the pool full the shard is refused with a typed transport exception and not left waiting.
- AC-9: with the setting off the action is not registered and the Java request gets `action_not_found`; with it on the same Java-built request is served.
- AC-10: in the wide mixed-fleet soak (`crates/node/tests/mixed_fleet/soak_wide.rs`, whose judge sends `match_all`, inside the subset) every answer of every node has `_shards.failed == 0` within `HANDOVER_SLACK_SECS` (60 s) of the judge starting, after a Java node is killed.
- AC-11: write p99 on the owner stays within 5% of its value without forwarded searches while forwarded searches run at the pool's limit (provisional, emulator); the default of the switch follows this result.

## S3 request budget

A row in `docs/request-budgets.md`: a forwarded search makes no blob request. A test spends it.

## Switch

`serverless.search.shard_action.enabled`, default on if AC-11 holds and off if not; see the ADR.

## Out of scope

Sort other than score, aggregations, highlight, rescore, `search_after`, collapse, suggest, script fields and PIT. Each is a later slice with its own rows.
