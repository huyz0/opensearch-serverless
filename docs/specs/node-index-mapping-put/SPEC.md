# PUT /{index}/_mapping on the node

## Serves

Parity plan R7.4, index admin REST: adding fields to an index after it was created. Ported from `MappingUpdateHandler` (shell 429b4cbc); the decisions (body requirement, merge, unchanged, statuses, attempts) are [coordinator-mapping-update](../coordinator-mapping-update/SPEC.md); the read-back is [node-index-mapping-get](../node-index-mapping-get/SPEC.md).

## Change

`oss-node`'s `IndexService` gains `PUT` and `POST` on `/{index}/_mapping` and `/{index}/_mappings`: read the descriptor, plan the merge of the body over the stored mapping (`mapping_update::plan`), and swap the descriptor over the generation read, writing the merged mapping and moving `mapping_version` by one. A lost swap reads again and plans again, up to `ATTEMPTS` (4) times; then 409 `status_exception`, "the mapping for X is being changed concurrently; retry". Answers `{"acknowledged":true}`.

**Concrete names only, as the Java**: it validates the name as an index or alias name and reads one descriptor; an alias, a list and a wildcard are 404 `index_not_found`, `_all` is 501. No body is 400 `missing_body` "a mapping body is required", before the index is read.

**Held shards.** A writer holding a shard of the index adopts the new mapping when its incarnation check next reads the descriptor (at most once a window, no extra request): see [shard adopts a changed mapping](../shard-adopts-mapping/SPEC.md).

## Acceptance criteria

- AC-1: a body adding a field to an index answers 200 `{"acknowledged":true}`, the stored mapping holds the field and its old ones, `mapping_version` is one higher, and the other descriptor fields are unchanged; `POST` and `/_mappings` do the same.
- AC-2: a body that changes nothing answers 200 and writes nothing (no swap, the version unchanged).
- AC-3: a body that conflicts with a stored field (a changed type) is 400 with the merge's message and writes nothing; a mapping that does not parse is 400.
- AC-4: no body or an empty one is 400 `missing_body` before any request to the store; a name nothing holds, an alias, a list and a wildcard are 404; a name beginning with an underscore is 501.
- AC-5: a swap lost to another writer reads again and publishes over what it finds, so two updates of different fields both keep theirs; four lost swaps are 409.
- AC-6: an update costs one register read and one swap; one that changes nothing costs the read.
- AC-7: with security on the route needs `indices:admin/mapping/put` on the index and has a row in the action table.
- AC-8: the body is read as it streams in, within a bound of 1 MiB; one byte more is 400.

## Test plan

`crates/node/tests/server/index_route/mapping_put.rs` over the emulator; the lost swap by a store that moves the register between the read and the swap; the security row in `tests/server/security_authz.rs`.

## S3 request budget

One register read and one swap, each lost swap adding one of each; the same cost as `MappingGrowth`.

## Bounds

The body is at most 1 MiB; the descriptor buffer is charged to the request-body pool as the GET's is.

## Switch

None: the Java has none.

## Risks

No live recording of the Java's answers; the wording comes from the source and the ported plan.
