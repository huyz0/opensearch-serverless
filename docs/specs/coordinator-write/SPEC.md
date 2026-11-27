# The coordinator decides where a write is applied and which native write options it accepts

## Serves

Parity plan R7.3, document APIs, second piece: the decisions in the Java shell's `DocumentWrites.written` and `validateNativeWrite` (ops package, shell 429b4cbc) that need no node. The writes themselves, the activation wait, the forward over the transport and the retry after a failed forward (already `Router::after_forward_failure`) are the node assembly's.

## Change

`oss-coordinator` gains `docs::write`: `plan_write` (where a write goes), `settle` (which placement stands after an activation), and `validate_native_write` (the options the native client may send).

## Acceptance criteria

- AC-1: a write at a placement this node holds is applied here (`WritePlan::Local`), whoever the head names: the writer here is the one that acknowledges.
- AC-2: a placement not held here whose owner is nobody or this node, not yet activated, is `WritePlan::Activate`: the caller asks the node to take the shard and places again.
- AC-3: the same placement after the activation was tried is refused with `Router::not_here`'s words (no owner: "no node currently owns shard N of i", not retryable; this node: "this node is acquiring shard N of i; retry"), doubting ownership.
- AC-4: a placement not held here whose owner is another node is forwarded to that owner (`WritePlan::Forward`), with no activation asked.
- AC-5: `settle` gives the placement made after an activation only when it is for the same index uuid; otherwise the one before stands (an index recreated meanwhile is a new routing decision).
- AC-6: `validate_native_write` accepts the internal version type with no pinned version, and `create` with the "deleted" pin; refuses any other version or version type with "version/version_type is not supported; use if_seq_no and if_primary_term"; and refuses a `wait_for_active_shards` above one with "wait_for_active_shards cannot exceed one: shards have no replicas", the version check first.

## Test plan

`crates/coordinator/tests/it/write.rs`: an in-memory plane recording doubts; the words asserted exactly.

## S3 request budget

Zero: the decisions read nothing; the activation and forward they name are metered where they are made.

## Bounds

None to hold: one decision per write.

## Switch

None: the shell's write path has no setting and no measured downside.

## Risks

A held placement whose head names another node is applied here, as the shell does; unlike a get, a write has no lagging-read problem to avoid, and the log's fencing decides whether the writer here may still acknowledge it.
