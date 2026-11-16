# Routing: every node puts a document in the same shard

## Serves

Roadmap R4.3 ([plan](../../roadmap.md)): which physical shard a document lives in, computed by Rust nodes exactly as Java nodes compute it, so a document one writes the other finds. [ADR 0015](../../adr/0015-cluster-crate-and-node-leases.md) (the `oss-cluster` crate); the unit layout is a field of the index descriptor every request already reads.

## Change

- `oss-format` gains `UnitLayoutRecord` and `ShardMode`: the shard mode's unit layout, read and written byte for byte as the shell's `UnitLayoutFormat` writes it, bounded at 288 KiB and 4,096 units.
- `oss-cluster` gains `routing`, a port of the shell's `routing/RoutingHash`, `UnitLayout` and `ops/DocumentRouting`: the 64-bit routing hash (murmur3 x64 128 `h1` over UTF-8), the declared-shard slices of the hash space, the checked layout (cover, no gap or overlap, the shape rule, a pending split), lookups, split planning, pending splits, apply and merge; and for an index without a shard mode, core's `generateShardId` (32-bit murmur3 over UTF-16, routing partition offset, routing shards equal to shards as the shell renders them).
- Per-index limits: `UnitLayout::within_shard_limit` answers whether the layout's slots fit the descriptor's 4,096; a legacy index needs a partition size below its shard count, as core does.
- Not changed: no node routes documents yet (the writer and coordinator will). The `_routing.required` mapping check stays with the caller that holds the mapping; an index with `index.number_of_virtual_shards` is not routed here.

## Acceptance criteria

- AC-1: `routing_hash` and `declared_shard` equal the Java's for each of 10,000 recorded ids (empty, ASCII, numbers, UUIDs, accented, CJK, surrogate pairs, up to 300 characters) at eight declared counts up to 4,096, and the first and last hash of every declared shard at eight counts equal `firstHashOf` and `lastHashOf`.
- AC-2: every one of the 10,000 ids routes to the Java's shard in a legacy index (twelve shard and partition pairs, with and without a routing value) and to the Java's unit in a fixed layout, a grown adaptive layout and a fixed layout with a declared shard halved; a partitioned index without a routing value is refused.
- AC-3: each of 36 recorded layouts reads and writes back byte for byte, and for each `unit_for` (2,185 hashes), `plan_split` of every unit, `units_of` with `needs_filter` for every declared shard, every neighbouring merge, and clearing or applying a pending split equal the Java's.
- AC-4: search routings split and dedupe as core's, and a legacy index with a routing partition is searched whole.
- AC-5: every layout text the Java refuses (32 of 47) is refused, every other reads to the Java's layout, except three the Java takes only by coercing a JSON type it never writes, which are refused; a record past 4,096 units or 288 KiB is refused, and a layout at 4,096 slots fits.

## Test plan

- AC-1 to AC-5: `crates/cluster/tests/routing/{hashes,layouts}.rs` over `fixtures/routing/`, recorded by `Routings.java` from the shell's classes compiled unchanged and core's `OperationRouting`. Fails if a hash, slice bound, lookup, cut point, merge rule or format byte differs.
- Mutants over the diff; planted defect: the hash's tail read big-endian.

## S3 request budget

| Operation | Requests |
|---|---|
| route a document, plan or apply a split | 0 (the layout comes with the descriptor) |

## Switch

None: a port with nothing to compare against, making no requests.

## Risks

- A hash or cut point that differed from the Java's would put a document where the other implementation never looks, and a search would return nothing rather than fail. AC-1 to AC-3 compare every recorded answer.
- A Java string with a lone surrogate hashes as `?`; a Rust `&str` cannot hold one, so such an id must be refused at the REST edge, as JSON parsing already does.
