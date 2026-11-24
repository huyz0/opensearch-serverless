# A writer finds its indexes in the descriptors

## Serves

R7.6, node assembly: `Routes::index` for a writer node, read from the descriptors the index create route writes, so a created index takes writes with no caller handing the node a table. The cluster half (who owns a shard, forwarding) is a separate piece; here every shard is this node's.

## Change

`oss-node` (feature `writer`) gains `DescriptorRoutes::new(descriptors, buffer_bytes)`, a `Routes`:

- `index(name)` reads the name's register (one read) and answers the index's `IndexInfo`: its shard count and uuid, `index.routing_partition_size` and `index.number_of_routing_shards` from its settings (flat or nested, number or string; 1 and the shard count when unset), and whether its mapping sets `_routing.required`. An alias, a tombstone, an absent name and a descriptor that cannot be read are `None`.
- `owner` is always `Owner::Local`; `forward` is never called for it and fails every item if it is, with 503 `no_owner`.

## Acceptance criteria

- AC-1: after an index is created with the create route, `index(name)` answers its name, uuid and shard count, with the routing defaults for an index created without settings.
- AC-2: `index.routing_partition_size`, `index.number_of_routing_shards` and `_routing.required` are read from a descriptor that sets them, in flat and nested settings and as numbers or strings.
- AC-3: an alias, an absent name and a malformed register answer `None`.
- AC-4: a lookup costs one read of the register and no LIST.
- AC-5: `owner` is local for every shard, and `forward` answers a 503 `no_owner` failure per item.

## Test plan

`crates/node/tests/roles/descriptor_routes.rs` over the emulator.

## S3 request budget

One read of the descriptor register per `index` call; no cache in this piece, so a bulk costs one read per distinct index it names. LIST 0.

## Risks

- One read per index per bulk is a cost that scales with requests; a descriptor cache fed by the change stripes is the next piece and must not delay seeing a create.
