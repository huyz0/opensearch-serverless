# Request cost header

## Serves

[ADR 0061](../../adr/0061-a-request-reports-its-own-store-requests.md), the second piece, after [request-cost-scope](../request-cost-scope/SPEC.md): a request that sends `X-Opensearch-Serverless-Cost: report` is answered with an `X-Opensearch-Serverless-Cost` header counting the store requests made for it on this node. Reads first; a write's header carries the requests its own threads made, and the commit window's append is a later change (`shared=log`).

## Change

- `oss_rest::cost` (`CostScope`, `CostSession`) and `Router::with_cost_scope`: for a request that sent the header, after the guard and the authorizer have let it through, the server begins a session (its scope is entered on the worker) and writes the session's value into the response head; the scope ends when the response is dropped.
- The node installs it when it has a usage ledger (store usage on), as `x-opensearch-serverless-cost: get=12;get_range=3;head=0;put=0;copy=0;list=0;delete=0;bytes_read=48211;bytes_written=0;units=1.040;node=n1;scope=node`: counts rounded to whole requests, `units` in PUT-equivalents.
- The cold open's file reads (`oss_reader::prefetch::primed`), the module load beside a commit's open (`search_open`) and the coordinator fan-out's helper threads capture the scope and enter it on their threads.
- The streaming `_msearch` writes its head before its sub-searches run and carries no header.
- Not in this change: the commit window's null scope and `shared=log` on writes, a request that is forwarded to another node.

## Acceptance criteria

- AC-1: on a node with store usage on, a request with the header gets the cost header, and the same request without it does not.
- AC-2: a request that reads the store (a search that opens its shard) reports more than zero units; its `node` is the node's id and its `scope` is `node`.
- AC-3: the value names every kind, the bytes, the units to three places, the node and the scope, in that order.
- AC-4: a node with store usage off sends no cost header.
- AC-5: an `_msearch` that asks gets no cost header.
- AC-6: reads made on the helper threads of a cold open's prefetch are counted in the request's scope.
- AC-7: the fan-out's helper threads count into the request's scope.

## Test plan

`crates/node/tests/server/boot_usage.rs` (AC-1, AC-2, AC-4, AC-5), `crates/node/src/cost_header.rs` (AC-3), `crates/reader/src/prefetch/primed.rs` tests (AC-6), `crates/coordinator/tests` fan-out (AC-7). Each seen failing with the behaviour removed.

## S3 request budget

Unchanged: a counter add per request already made; a request without the header adds nothing.

## Switch

The request header; no node setting ([ADR 0061](../../adr/0061-a-request-reports-its-own-store-requests.md)).

## Risks

- A hand-off site that is missed under-counts; the three on a read path are covered above, and the module-load site has no test of its own beside the cold open's.
