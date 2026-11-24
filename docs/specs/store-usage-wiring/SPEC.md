# Store usage wiring

## Serves

Roadmap R13.3, second half: the ledger of [store-usage](../store-usage/SPEC.md) on a running node, and the shell's `GET /_serverless/usage`.

## Change

- `OSS_STORE_USAGE` (default `false`) wraps the node's store in a `UsageStore` over a `Ledger` of at most `OSS_STORE_USAGE_MAX_INDICES` (default 1,000, 1 to 1,000,000) indices. Keys are classified by `oss_format::index_of_key` under the node's base.
- With it on, `GET /_serverless/usage` answers `node_id`, `started_millis`, `now_millis`, `metered` and `indices`: each index, then `_node` and `_other`, with `cost_units` (PUTs; a GET counts 0.08), `requests` by kind (decimals: a shared request is a fraction) and `bytes` read and written. Counters only grow, so a reader takes the difference of two reads.
- With it off, the route is not registered and the store is not wrapped.
- Not in this change: splitting node-log and pack uploads across the indices they carry (they read as `_node`), the opt-in cost header on responses, and tenant roll-up and limits (R13.4).

## Acceptance criteria

- AC-1: `OSS_STORE_USAGE` is off by default; `OSS_STORE_USAGE_MAX_INDICES` defaults to 1,000 and is refused at 0 or above 1,000,000.
- AC-2: a node started with it on, after creating an index and writing a document, answers the route with its id, `metered:true`, a line for the index, and `_node` and `_other` lines.
- AC-3: a node started without it serves no usage route.
- AC-4: the route's body lists every request kind and writes thousandths as decimals (`1.250`).

## Test plan

- AC-1: `store_usage_is_off_by_default_and_bounded` (node config tests), seen red before the fields existed.
- AC-2, AC-3: `tests/server/boot_usage.rs`, over a file store and real sockets.
- AC-4: `tests/usage_route.rs` over the emulator.

## S3 request budget

Unchanged: no request is added. Off by default, the node does exactly what it did.

## Switch

`OSS_STORE_USAGE`, default off. No S3 request is added and memory is bounded by the index limit, but each store attempt now takes two atomic adds and a thread-local read, and the write p99 effect is not measured here. It turns on by default once a run shows write p99 within 5% with it on.

## Risks

- Until uploads that carry many indices are split, the node-log's cost reads as `_node`: the per-index lines under-count writes.
- An index past the limit is counted in `_other`, so a tenant's bill needs the limit above the indices the node serves.
