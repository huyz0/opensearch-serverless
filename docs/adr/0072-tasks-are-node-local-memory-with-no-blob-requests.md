# 0072: Tasks live in node memory, bounded, with no blob requests

**Status:** Accepted after independent review, 2026-11-27 (changes folded in)

## Context

`wait_for_completion=false` on `_update_by_query`, `_delete_by_query` and `_reindex` returns a task id at once; the client then reads `GET /_tasks/{id}` and may `POST /_tasks/{id}/_cancel`. OpenSearch keeps a running task in the coordinating node's memory and, when it finishes, writes its result to the `.tasks` system index. In this engine a result write would be a blob PUT per task and a read a GET, for a convenience the caller reads once or never, and an index is not a place a serverless node keeps state ([principles](../principles.md): requests scale with nodes and bytes, idle indices make zero requests).

## Decision

Tasks are a per-node in-memory registry in `oss-node`. No blob is read or written for a task, there is no timer, and nothing is listed.

- A task id is `<node_id>:<n>`, `n` starting at the process's start time in milliseconds and counting up, so an id never names a different task after a restart (the old id is then 404). `GET /_tasks/{id}` and `_cancel` answer only for this node's ids. An id of another node is 404 `resource_not_found_exception` naming the node, so a client or balancer that holds the id's node can route to it. A node that receives an id naming another node sends the read or cancel to it over the node-to-node transport (`internal:serverless/tasks/call`, signed, to the lease this node already holds, a single hop that the target never repeats) and relays its answer; this costs no S3 request. With no live lease for the node the id is 404.
- Bounds: at most 32 running tasks (a 33rd request is 429 `es_rejected_execution_exception`; each runs on its own thread, so the bound is also a thread bound) and the 128 most recently finished kept. A task's whole stored entry (`task` object with its description, response and error) is capped at 64 KiB: a larger response keeps its counts and drops its `failures`, a longer description or error is truncated. Each task, running or kept, holds a 64 KiB reservation from the `RequestBodies` pool until it is dropped (a refused reservation is a 429), so the registry is at most 160 x 64 KiB = 10 MiB of the node's budget and never beyond it. A finished task is also dropped once it is 1 hour old. Limits are checked when a task is added or read, never by a timer.
- A walk thread frees its slot and records the task as failed even when it panics (a drop guard), so a slot cannot leak.
- Cancel is cooperative: it sets a flag the walk reads before each page, so a cancelled task stops within one page, at most the walk's `BATCH_SIZE` (1,000) documents. Documents already written stay written. Walk threads are detached: shutdown does not wait for them, and the process exit ends them as it ends any in-flight request.
- `GET /_tasks` lists running tasks only, as OpenSearch does; a finished task is reachable by id only. Synchronous requests are not tasks. A running task's `GET` shows no `status` (counts so far), because the walk reports counts when it ends; the finished `response` carries them. This and the points above are recorded in `docs/compatibility.md`.
- A task's result is lost when the node restarts; a task running on a node that dies is simply gone. This differs from OpenSearch's `.tasks` index.
- No setting: the registry costs a few kilobytes idle and adds no request; there is no downside to measure.

## Consequences

- Easier: zero added S3 requests, no new durable format, no fencing question; idle nodes spend nothing.
- Harder: a result is not readable from another node or after a restart; a client must poll the node that accepted the request (the id names it).
- Rules out: `GET /_tasks` as a cluster-wide listing (this node's tasks only) and `wait_for_completion` on a task after the node is gone.
- Later, if a durable result is wanted, it would be a separate ADR weighing one PUT per finished task against the benefit.
