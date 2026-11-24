# A node creates an index and says whether one exists

## Serves

R7.4 index admin, node wiring: `PUT /{index}` and `HEAD /{index}` over the descriptor store. Follows [coordinator-index-create](../coordinator-index-create/SPEC.md) and [coordinator-index-admin](../coordinator-index-admin/SPEC.md), which decide what a request means. Delete, GET, mapping and settings routes, and aliases at create, are separate pieces: delete needs shard heads and pins that nothing implements yet ([ADR 0012](../../adr/0012-index-deletes-and-reclaim.md)).

## Change

`oss-node` gains `index_route` (features `writer` and `coordinator`): `IndexService::new(descriptors, clock, buffer_bytes)` and `routes(router, service)`.

- `PUT /{index}` reads the body (at most 1 MiB, its growth reserved from the request-body pool), parses it with `oss_coordinator::indices::create::parse`, checks the mapping parses as a mapping, mints a uuid, and creates the descriptor with one conditional create. It answers `create_ack`.
- `HEAD /{index}` answers 200 when the name holds an index or a published alias and 404 when it holds nothing, with no body.

Refused with 501 `unsupported_index_option`: settings other than the shard and replica counts, and aliases, at create. A name the descriptor store refuses is 400 `invalid_index_name_exception`; one taken is 400 `resource_already_exists_exception`; one whose tombstone was just claimed is 409 `name_being_reclaimed_exception`; a mapping that does not parse is 400 `mapper_parsing_exception`.

## Acceptance criteria

- AC-1: a create writes a descriptor with the body's shard count and mapping, a 22-character uuid, versions 1 and the clock's time, and answers 200 with `create_ack`; a body that is only a mapping is the mapping.
- AC-2: a second create of the name is 400 `resource_already_exists_exception` and the first descriptor is unchanged.
- AC-3: a name beginning with an underscore is 501, one beginning `.ds-` is 400, and a name no index may have is 400 `invalid_index_name_exception`, each before any request to the store.
- AC-4: settings other than the shard and replica counts, and aliases, are 501 and write nothing; a malformed body, an out-of-range shard count and a mapping that does not parse are 400 and write nothing.
- AC-5: HEAD answers 200 for an index, 200 for a published alias and 404 for a name nothing holds, with an empty body.
- AC-6: a create of a new name costs one conditional create of the register; a HEAD costs one read; LIST 0.
- AC-7: the descriptor buffer is charged to the request-body pool: a pool without room for it answers 429 and writes nothing.
- AC-8: a body of exactly 1 MiB is read and one byte more is 400, writing nothing.
- AC-9: a descriptor that does not fit the buffer is 400 `illegal_argument_exception`, not a failure, and writes nothing.

## Test plan

`crates/node/tests/server/index_route.rs` over the emulator.

## S3 request budget

Create: one conditional create of the descriptor register (a taken name costs one more read). HEAD: one read. LIST 0.

## Risks

- A create writes no shard heads or commits: a shard opens on its first write.
