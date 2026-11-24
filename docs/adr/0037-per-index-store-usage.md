# 0037: The store counts its requests per index in a ledger beside the meter

**Status:** Accepted after independent review of the change, 2026-11-24

## Context

The meter counts requests for the whole node. Tenant limits and a bill by tenant need the same counts by index. The shell keeps them in `StoreUsage`: a request goes to the index its key names, a shared upload is split by the weight of the indices in it, and the rest is the node's.

## Decision

- A ledger keeps requests (in thousandths) and bytes per index, bounded by `max_indices`, with the rest in `_other` and keyless requests in `_node`.
- The meter gains an optional tap so the ledger sees exactly the attempts the meter counts, retries included. A wrapper store names the owner from the key; a thread-local carries it to the tap, so no store signature changes.
- Keys are classified by `oss-format`, which owns their layout.
- Every request lands on one line, so the lines add up to the meter.

## Consequences

- No request is added. Cost per attempt is two atomic adds and a thread-local read.
- Memory is bounded by `max_indices` counter sets.
- A thread that hands work to another loses the owner, and that work reads as node cost until the pool carries it; the totals stay right.

## Wiring

`OSS_STORE_USAGE` (default off) wraps the node's store and serves `GET /_serverless/usage`; see [store-usage-wiring](../specs/store-usage-wiring/SPEC.md). It stays off until a run shows write p99 within 5% with it on: it adds no request, but each attempt now takes two atomic adds and a thread-local read.
