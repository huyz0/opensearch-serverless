# Tasks for by-query and reindex: wait_for_completion=false, GET and cancel

## Serves

The by-query and reindex follow-up: `wait_for_completion=false` on `_delete_by_query`, `_update_by_query` and `_reindex`, with `GET /_tasks`, `GET /_tasks/{id}` and `POST /_tasks/{id}/_cancel`. Decision: [ADR 0072](../../adr/0072-tasks-are-node-local-memory-with-no-blob-requests.md). Builds on [node-delete-by-query](../node-delete-by-query/SPEC.md), [node-update-by-query](../node-update-by-query/SPEC.md) and [node-reindex](../node-reindex/SPEC.md).

## Change

- `oss-node`: a `tasks` registry (in memory, bounded as the ADR says) and the routes. A request with `wait_for_completion=false` registers a task, runs the walk on its own thread, and answers 200 `{"task":"<node_id>:<n>"}` at once. Validation (parameters, body, missing index, switch) runs before the task starts, so those refusals stay synchronous.
- `GET /_tasks/{id}`: `{"completed":false,"task":{...}}` while running; when finished `{"completed":true,"task":{...},"response":{...}}` with the answer the synchronous call would have sent, or `"error":{...}` when it failed. The `task` object holds `node`, `id`, `type` `transport`, `action` (`indices:data/write/delete/byquery`, `.../update/byquery`, `.../reindex`), `description`, `start_time_in_millis`, `running_time_in_nanos`, `cancellable: true` and `cancelled`.
- `POST /_tasks/{id}/_cancel`: sets the flag and answers `{"nodes":{"<node>":{"tasks":{"<id>":{...}}}}}`; a finished task is 404.
- `GET /_tasks`: this node's tasks, running and kept, as `{"nodes":{"<node>":{"tasks":{...}}}}`; `actions` filters by exact action name or `*` suffix. Other parameters are refused 400.
- A cancelled task finishes with the counts so far and `"canceled":"by user request"` in its response; the task's `completed` is true.
- Refused: a task id of another node (404 naming it), a malformed id (400), a 33rd running task (429), `wait_for_completion` on `GET /_tasks/{id}` (400, unsupported).
- Not served: cluster-wide listing, `detailed`, `group_by`, `parent_task_id`, `timeout`, `POST /_tasks/_cancel` without an id, a result after a restart.

## Acceptance criteria

- AC-1: `wait_for_completion=false` returns a task id at once and the work finishes in the background; `GET /_tasks/{id}` then answers `completed: true` with the same counts the synchronous call gives, and for `_reindex`, `_update_by_query` and `_delete_by_query` alike.
- AC-2: while the task runs `GET /_tasks/{id}` answers `completed: false` with the task's action and description, and `GET /_tasks` lists it, filtered by `actions`.
- AC-3: cancel stops a running task within a page, the task reports `canceled` and the documents of the pages already done; cancelling a finished or unknown task is 404.
- AC-4: a failing task (script error) is `completed: true` with `error`; a refusal found before the task starts (missing index, bad parameter, switch off) is still the synchronous status.
- AC-5: bounds: a 33rd running task is 429; only the 128 newest finished tasks are kept; a finished task older than the TTL is gone; an answer over 64 KiB keeps its counts and drops its failures.
- AC-6: an id of another node is 404 naming it, a malformed id 400, and a task makes no blob request.

## Test plan

`crates/node/src/tasks.rs` unit tests for the registry bounds with an injected clock; `crates/node/tests/server/tasks_route.rs` over the by-query rig, the walk held on a gate the test opens so a running task and a cancel are observable.

## S3 request budget

None added: no task state is read or written to a blob; the walk's own requests are as in the by-query specs.

## Bounds

32 running (a thread each), 128 finished at 64 KiB each (8 MiB at most), a TTL of 1 hour checked on access.

## Switch

None (ADR 0072): the registry has no measured downside.

## Risks

- A result is readable only on the node that ran it and until restart or the TTL.
- A cancelled walk may finish the page it is in.
