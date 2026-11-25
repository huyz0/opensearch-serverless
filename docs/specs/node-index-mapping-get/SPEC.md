# GET /{index}/_mapping on the node

## Serves

Parity plan R7.4, index admin REST: the read-back of what an index's mapping holds, so a client (and the mixed-fleet soak) can see the fields dynamic mapping added ([dynamic mapping reaches the descriptor](../dynamic-mapping-durable/SPEC.md)). Ported from the `GET /{index}/_mapping` and `/_mappings` routes of `IndexAdminHandler` and `IndexDescriptorRendering.renderMapping` (shell 429b4cbc); the route's view selection is `oss_coordinator::indices::admin::view_of`.

## Change

`oss-node`'s `IndexService` gains `GET /{index}/_mapping` and `GET /{index}/_mappings`: one register read of the index's descriptor, answered as `{"<index>":{"mappings":<the stored mapping>}}`, or `{"<index>":{"mappings":{}}}` for an index with no stored mapping.

**Concrete names only, as the Java.** The Java reads `descriptors.get(index)`: an alias, a comma list, `_all` and a wildcard are not resolved on this route. A name that is not an index (an alias included, and a name with characters no index may have, such as a list or a wildcard) answers 404 `index_not_found`, "no such index: <name>". The HEAD route of this service answers 200 for an alias; this route follows the Java's GET, not that HEAD. Aliases, lists and wildcards on `_mapping` would be an extension of the Java shell and are not added here.

**Member order.** The Java re-serialises the stored mapping from a map, so its member order is the map's; this answers the stored text, member order as written. Clients compare parsed JSON.

## Acceptance criteria

- AC-1: for an index created with a mapping, GET answers 200 with `{"<index>":{"mappings":<the mapping>}}`; `/_mappings` answers the same; the body parses to what was stored.
- AC-2: for an index with no stored mapping the answer is `{"<index>":{"mappings":{}}}`.
- AC-3: a name nothing holds, and an alias, answer 404 `index_not_found` "no such index: <name>", a comma list and a wildcard too; a name beginning with an underscore is 501 before any request to the store.
- AC-4: a field a write added by dynamic mapping is in the answer once the write is acknowledged, on every node.
- AC-5: a GET costs one register read and nothing else.
- AC-6: the descriptor buffer is charged to the request-body pool as the HEAD's is; the parameters the Java consumes without effect (`local`, `include_defaults`, `ignore_unavailable`, `allow_no_indices`, `expand_wildcards`, the timeouts, `wait_for_active_shards`, `include_type_name`, `flat_settings`) are accepted.

## Test plan

`crates/node/tests/server/index_route/mapping_get.rs` over the emulator for AC-1 to AC-3, AC-5 and AC-6; AC-4 extends `crates/node/tests/dynamic_mapping.rs`.

## S3 request budget

One register read, the same as HEAD.

## Bounds

The descriptor buffer, as the HEAD's.

## Switch

None: a read of what is stored, with no downside.

## Risks

No live recording of the Java's answer was made for this piece; its shape is taken from the source above.
