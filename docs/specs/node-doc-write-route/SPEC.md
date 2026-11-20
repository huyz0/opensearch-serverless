# A writer node serves the single-document write APIs

## Serves

Parity plan R7.6, node assembly: `PUT|POST /{index}/_doc/{id}`, `POST /{index}/_doc`, `PUT|POST /{index}/_create/{id}` and `DELETE /{index}/_doc/{id}`, answered as stock OpenSearch answers them. Each is one item run through the same bulk machinery as `_bulk` ([node-bulk-route](../node-bulk-route/SPEC.md)), so planning, version rules, the write log and shard routing are one path. Get, update and the ingest pipeline are separate pieces.

## Change

- `oss-writer`'s bulk module gains `write_doc` and `doc_status`: one item rendered as the single-document answer (`_index`, `_id`, `_version`, `result`, `_shards`, `_seq_no`, `_primary_term`, or `error` and `status`) and its HTTP status.
- `oss-node`, with the `writer` feature, gains `doc_route`: `DocService` and `routes`. The body is held whole, charged to the request-body pool before it is copied.
- Parameters read: `refresh`, `routing`, `op_type`, `version`, `version_type`, `if_seq_no`, `if_primary_term`, `pipeline`, `require_alias`.

## Acceptance criteria

- AC-1: `PUT` and `POST` of `/{index}/_doc/{id}` write the document: 201 `created` with version 1 and `_shards` total 1, then 200 `updated` with version 2 for the same id.
- AC-2: `POST /{index}/_doc` gives the document a generated id and answers 201.
- AC-3: `PUT /{index}/_create/{id}` and `?op_type=create` refuse an id that has a live document with 409 `version_conflict_engine_exception`.
- AC-4: `DELETE /{index}/_doc/{id}` answers 200 `deleted` for a live document and 404 `not_found` for none, both with a version and sequence number.
- AC-5: `?refresh=true` adds `forced_refresh`, any other refresh value but `false`, `wait_for` or empty is 400, and a stale `if_seq_no` is 409.
- AC-6: a document the mapping refuses answers its status (400 `mapper_parsing_exception`) as `{"error":{...},"status":400}`, and an index the node has no route for answers the item's failure.
- AC-7: a generated id carries the write's time, and `?op_type=create` refuses a live id as `_create` does.
- AC-8: the document is charged to the request-body pool once when its length is declared, and as it is read when it is not (a refusal when the pool is full).
- AC-9: `doc_status` and `write_doc` in `oss-writer` give each outcome its status (201 created, 200 updated or deleted, 404 not found, a failure its own) and the single-document answer.
- AC-10: `?require_alias=true` on an unknown index is refused naming the flag.

## Test plan

`crates/node/tests/doc_route.rs` over the emulator and a real server socket, as `bulk_route.rs`.

## S3 request budget

One write staged in the node's log as for any bulk item; the route adds no request.

## Risks

- A document is held whole (as the pool reservation counts it); `_bulk` is the streaming path for large volumes.
