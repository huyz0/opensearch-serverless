# 0072: Tasks live in node memory, bounded, with no blob requests

**Status:** Proposed

## Context

`wait_for_completion=false` on `_update_by_query`, `_delete_by_query` and `_reindex` returns a task id at once; the client then reads `GET /_tasks/{id}` and may `POST /_tasks/{id}/_cancel`. OpenSearch keeps a running task in the coordinating node's memory and, when it finishes, writes its result to the `.tasks` system index. In this engine a result write would be a blob PUT per task and a read a GET, for a convenience the caller reads once or never, and an index is not a place a serverless node keeps state ([principles](../principles.md): requests scale with nodes and bytes, idle indices make zero requests).

## Decision

Tasks are a per-node in-memory registry in `oss-node`. No blob is read or written for a task, there is no timer, and nothing is listed.

- A task id is `<node_id>:<n>`, `n` a per-process counter. `GET /_tasks/{id}` and `_cancel` answer only for this node's ids; an id of another node is 404 `resource_not_found_exception` naming the node, so a client or balancer that holds the id's node can route to it.
- Bounds: at most 32 running tasks (a 33rd request is 429 `es_rejected_execution_exception`; each runs on its own thread, so the bound is also a thread bound) and the 128 most recently finished kept, each result at most 64 KiB (a larger answer keeps its counts and drops its `failures` list). A finished task is also dropped once it is 1 hour old. Both limits are checked when a task is added or read, never by a timer.
- Cancel is cooperative: it sets a flag the walk reads before each page, so a cancelled task stops within one page (at most 1,000 documents) and reports what it did. Documents already written stay written.
- A task's result is lost when the node restarts; a task running on a node that dies is simply gone. This differs from OpenSearch's `.tasks` index and is documented in the compatibility notes.
- No setting: the registry costs a few kilobytes idle and adds no request; there is no downside to measure.

## Consequences

- Easier: zero added S3 requests, no new durable format, no fencing question; idle nodes spend nothing.
- Harder: a result is not readable from another node or after a restart; a client must poll the node that accepted the request (the id names it).
- Rules out: `GET /_tasks` as a cluster-wide listing (this node's tasks only) and `wait_for_completion` on a task after the node is gone.
- Later, if a durable result is wanted, it would be a separate ADR weighing one PUT per finished task against the benefit.
