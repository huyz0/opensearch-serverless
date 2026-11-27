# A node serves points in time over REST

## Serves

R10.6: the create, delete and list routes of a point in time, in the Java's PointInTimeHandler spellings. Follows [point-in-time-capture](../point-in-time-capture/SPEC.md) and [point-in-time-store](../point-in-time-store/SPEC.md).

## Change

`oss-node`, with the `writer` feature (which holds `oss-meta`), gains `pit_route`: `PitService` and `routes`. A node's assembly registers it with `pit_route::routes(router, service)`; **no assembly does yet**, so a running node answers 404 for these paths until R7 wires it (the module's registration is the one line this change adds to `lib.rs`).

- `POST /{index}/_search/point_in_time` and `POST /{index}/_pit`: freezes the one concrete index. `keep_alive` (a time value, default 5m, at most 1h; more is refused, not clamped) sets the absolute deadline. Answers `pit_id`, `_shards` (total and successful the shard count, none skipped or failed), `creation_time`, `index` and `keep_alive_millis`.
- `DELETE /_search/point_in_time` with a body naming `pit_id` (one id or an array, `_all` meaning every view), `DELETE /_search/point_in_time/_all`, `DELETE /_search/point_in_time/{id}` and `DELETE /_pit/{id}`: release views and answer `pits`, one `{successful, pit_id}` an id, a repeated id once. Naming ids of which none existed is 404 `pit_not_found`. `_all` releases the views that are finished, never a capture still running.
- `GET /_search/point_in_time/_all`: `pits`, one `{pit_id, keep_alive (milliseconds left), index}` a finished view.

Refusals as the Java's: a list or pattern of indices, an alias, a bucketed index and `routing` are 501 `unsupported_point_in_time` (`preference` is taken and has no effect: a shard has no copies); an absent index is 404 `index_not_found`; a shard that published nothing is 409 `nothing_published`.

Not here: closing the frozen shards this node holds open for a view (there is no cache of them yet, R7), searching a view, the system-index refusal, and the janitor pass reading the views.

## Acceptance criteria

- AC-1: a create answers 200 with a pit_id that reads back from the store as a finished view of every shard, and the shard counts, creation time, index and keep-alive in the answer.
- AC-2: `keep_alive` takes ms, s, m, h and d units; a value past an hour is 400 `keep_alive_too_long`, and one that is no time value is 400 `illegal_argument_exception`; neither writes anything.
- AC-3: a pattern or list of indices, an alias, a bucketed index, and `routing` are 501; an absent index is 404; a shard with nothing published is 409 and leaves nothing live.
- AC-4: a delete by body (an id, an array, with `_all`), by `_all` and by path releases the views and answers each id; ids none of which existed are 404; a body with another field, no body, or an id that is no id is refused or not found, never a 500.
- AC-5: `_all` releases finished views and leaves a capture in progress.
- AC-6: a list answers each finished view with its remaining keep-alive and index, and nothing for a capture in progress, a released view or an expired one.
- AC-7: the literal `_all` path wins over `{id}`.
- AC-8: the time units nanos, micros, ms, s, m, h and d parse as `TimeValue` does (a bare `0` too, a value that overflows is 400); an empty `routing` or `preference` is no refusal; a day is refused by the cap, not the parse.
- AC-9: a delete body names at most 1,024 ids of at most 256 bytes each, an empty array, a repeated `pit_id` and any other field are 400, and a repeated id is released once.
- AC-10: a request whose buffer the request-body pool cannot hold is 429 and writes nothing; a view larger than the buffer is a 500 `point_in_time_failure` that leaves nothing pinned.
- AC-11: requests are exact: a refused keep-alive sends none, a create of two shards is 5 GETs, 1 HEAD and 3 PUTs, a delete one GET, PUT and DELETE, a list one LIST and a GET a view.

## Tests

`crates/node/tests/pit_route.rs` over the emulator and a real server socket.

## S3 request budget

As the store's: a create is the capture's budget, a delete one read, one swap and one delete per id, `_all` and a list one LIST page per 1,000 views plus a GET each (the listing reads every live view whole; a reduced form comes if deployments keep many large views).

## Risks

- The service holds one buffer a request, `buffer_bytes`, charged to the request-body pool. A view past it is a 500, by design: the assembly sizes the buffer to `oss_format::MAX_POINT_IN_TIME_BYTES` (16 MiB), the most a view may be, so a view that is too large for it cannot be stored in the first place. The 429 above is what a pool too small for that says.
- The alias refusal names no indices (`CaptureError::IsAlias` carries none), where the Java lists them.
- Checks run in the Java's order up to the keep-alive cap; a pattern or list of indices is refused after it, as `freeze` does.
