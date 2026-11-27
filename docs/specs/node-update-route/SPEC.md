# A writer node serves `POST /{index}/_update/{id}`

## Serves

Parity plan R7.3/R7.6: the update API over the shards this node writes. The route reads the document (the realtime get of [node-get-realtime](../node-get-realtime/SPEC.md)), decides with the planning of [coordinator-update](../coordinator-update/SPEC.md), and writes the result through the `_bulk` machinery as an index, create or delete item that carries the sequence number just read, so two concurrent updates cannot both win. A bulk `update` item stays refused: it needs the same read inside the writer crate and is another piece.

## Change

- `oss-node`, with the `writer` feature, gains `update_route` (the handler and its answer) and `update_body` (the body's keys and the merge of a `doc` into a source).
- `security_authz` maps the route to `indices:data/write/update`.
- Parameters read: `refresh`, `routing`, `retry_on_conflict`, `if_seq_no`, `if_primary_term`, `require_alias`. Any other parameter is refused.
- Body keys read: `doc`, `doc_as_upsert`, `upsert`, `script` (`source`, `lang` of `painless` or absent, `params`), `scripted_upsert`, `detect_noop` (default true). Another key is a 400.
- A `doc` is merged into the stored source as `XContentHelper.update` does: objects merge key by key, anything else replaces. With `detect_noop`, an unchanged merge writes nothing.
- A script runs by `docs::update_script::run_update`, so it needs `serverless.scripting.painless.native_compile`; with the switch off a script update is a 501.
- The answer is the single-document write answer, with `result` `created`, `updated`, `deleted` or `noop` (a noop has `_shards.total` 0 and the version read).
- A conflict on the write is tried again, up to `retry_on_conflict` more times, each time from a fresh read.

## Acceptance criteria

- AC-1: A `doc` merges into an existing document (nested objects key by key, other values replaced) and answers 200 `updated` with the next version.
- AC-2: `detect_noop` leaves an unchanged document alone and answers `noop` with the version read; `detect_noop:false` writes it.
- AC-3: A missing document is a 404 `document_missing_exception`, unless `doc_as_upsert` creates it from the doc or an `upsert` creates it from that (201 `created`).
- AC-4: A script edits `ctx._source` and a `ctx.op` of `delete` deletes and `noop` writes nothing; `scripted_upsert` runs the script over the upsert for a missing document. With the switch off a script update is a 501.
- AC-5: The caller's `if_seq_no` and `if_primary_term` are checked on the read (409 `version_conflict_engine_exception`), and `retry_on_conflict` below zero, `scripted_upsert` with no script, `require_alias` and an unknown body key or parameter are refused with the coordinator's messages.

## Test plan

`crates/node/tests/server/update_route.rs` over the emulator and a real server socket, as `doc_route.rs`; the script cases in the own-process binary `crates/node/tests/update_script_route.rs` because the switch is process-wide.

## S3 request budget

One realtime get and one staged write per attempt, as a get plus an index; the route adds no request of its own.

## Switch

Merge updates need none. Script updates are off unless `serverless.scripting.painless.native_compile` is on.

## Risks

The retry on a conflict (`retry_on_conflict`) has no test: it needs a write that loses a race between the read and the write, and the route has no hook for one. The expectations come from reading `UpdateHelper` and `TransportUpdateAction`, not a recorded run. The `_source` parameter, `wait_for_active_shards`, `timeout`, `if` conditions on pipelines and a stored script are not served and are refused as unknown parameters or keys.
