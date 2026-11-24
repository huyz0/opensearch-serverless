# Store usage

## Serves

Roadmap R13.3, the per-index meter the shell keeps in `StoreUsage` and `IndexUsage`, and invariant I-1 (every request metered). Tenant guardrails (R13.4) read it. [ADR 0037](../../adr/0037-per-index-store-usage.md).

## Change

- `oss-blob` gains `usage`: a `Ledger` of `IndexUsage` lines, one per index plus `_node` and `_other`, each holding requests by kind (in thousandths of a request) and body bytes read and written.
- `Meter` gains a tap: an optional observer told of each attempt and each byte count as the meter records it. The meter's own counters are unchanged.
- `UsageStore` wraps a store, installs the ledger as the meter's tap, and names the owner of each call from its key through a `Classifier`. A request is charged to the index its key names, else to the indices the thread works for (`Ledger::split`, by weight, remainder to the heaviest), else to `_node`.
- `oss-format` gains `index_of_key`, the classifier for the key layouts that name one index.
- Not in this change: wiring into the node, a route, a setting (the next change), and the per-tenant roll-up.

## Acceptance criteria

- AC-1: a request on a key an index owns is charged to that index (a batch delete to the index all its keys name); a key no index owns goes to `_node`.
- AC-2: the lines add up to the meter's count for every request kind, and to its bytes read and written.
- AC-3: bytes read and written are charged to the index that moved them.
- AC-4: work run under `split` shares each request and byte by weight, rounded down, the heaviest taking the remainder so the shares add up exactly. No weight above zero charges `_node`. A key's owner wins over a split. Outside the call the thread charges `_node` again.
- AC-5: past `max_indices` tracked indices, the rest are counted together as `_other`, and `lines()` lists indices sorted, then `_node`, then `_other`.
- AC-6: a line's cost is one unit a PUT-class request, 0.08 a GET-class request, and nothing a DELETE.
- AC-7: `index_of_key` gives the index for descriptor, tombstone, reclaim, segment and head keys, with or without a base, and `None` for node packs, node logs, leases, change stripes, cluster settings, an empty name or a key outside the base.

## Test plan

- AC-1 to AC-6: `crates/blob/tests/emulator/usage.rs`. Seen red by planting the remainder going to the lightest line and a GET priced at 0.8.
- AC-7: `crates/format/tests/key_owner.rs`. Seen red by planting a head key read whole as its index name.

## S3 request budget

Unchanged: no request is added. The tap adds an atomic add and a thread-local read to each attempt, and no allocation on the request path beyond the first sight of an index (`// ALLOC:` at the site).

## Switch

None in this change: the wrapper is unused until it is wired. The node-level switch, default off until measured, comes with the wiring.

## Risks

- A request made on a thread other than the caller's (a pool) is charged to `_node`, not lost; the lines still add up.
- A key layout that gains an index-bearing form needs a case in `index_of_key`, or its requests read as node cost.
