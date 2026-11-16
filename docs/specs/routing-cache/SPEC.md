# Routing cache

## Serves

The plan's metadata-writes milestone, eighth part. Every request names indexes, and resolving a name is one register GET. Today that GET is the whole store cost of a warm search and half a write's, for a fact that changes only when an operator changes it. Routing may answer from a read up to a second old, as the Java shell's does, and a node sees its own changes at once. Follows [alias-mutations](../alias-mutations/SPEC.md).

Invariants served:
- **S3 requests are the bill:** a name routed again within the window costs no request.
- **Bounded memory:** the cache holds register bytes under a byte budget, 64 MiB by default.
- **Fencing:** only routing reads the cache. Every compare-and-swap still reads its generation through.

## Change

Decision record: [ADR 0008](../../adr/0008-metadata-crate.md) (Accepted). No bytes change.

Ported from huyz0/OpenSearch at `3920dc36`:
- `serverless/shell/src/main/java/org/opensearch/serverless/metadata/RoutingCache.java`: `resolveForRouting`, `forgetRouting`, the prune, and the window, budget and threshold setters.
- `metadata/DescriptorStore.java`: `onChanged`.

### In `oss-meta`

- **`DescriptorStore::on_changed(listener)`** tells the listener the name of every record the store creates, updates or deletes, once the change applied. An alias's create and swap count, and so does a create over a tombstone. A refused or failed change does not.
- **`RoutingCache::resolve(descriptors, name, now, buf)`** answers as `DescriptorStore::resolve`.
  - It answers from a register read up to the window before (`DEFAULT_ROUTING_CACHE_MILLIS`, one second), copied into `buf` and parsed there, with no request.
  - A miss reads through. An absent name, a tombstone or an alias still being created is never kept, so a new index routes the moment it exists.
  - A malformed register is refused and not kept. A cached register longer than `buf` is read through, and refused as the store refuses it.
- **`RoutingCache::forget(name)`** drops a name, and the store's listener calls it, so this node reads its own change on its next request. Another node's change shows within the window.
  - Each forget moves an epoch, and a read that began before one is not kept, so it cannot put back what the change made stale.
- **Bounds:** an entry weighs its register's bytes, its name and 128 bytes.
  - A read that does not fit the budget drops expired entries first. If it still does not fit, it is not kept, so one large mapping never evicts the small ones.
  - Past `DEFAULT_ROUTING_CACHE_PRUNE_ABOVE` names (10,000), expired entries are dropped, at most once a window.
  - Lowering the budget below what the cache holds empties it.
- `set_window_millis(0)` reads through every time.

### Differences from the Java

- The cache keeps register bytes and parses on each hit, where the Java keeps the parsed record. Its weight is then the bytes it holds, not an estimate.
- A read past the budget evicts only expired entries, where the Java's cache evicts the least recently used.
- `readConfirmingIncarnation` comes with the write fences that use it.

### Not in this change

The callers: the coordinator and reader request paths route through this once they resolve names. The register map's other keys arrive with their owners (shard heads, leases, templates, snapshots), each in `oss_format`'s key module as it is ported.

## Acceptance criteria

- AC-1: A name read once routes with no request for the window, and reads through one millisecond past it, on each backend. A miss is never cached, and a name created elsewhere routes at once. A window of zero reads through every time. Tests: `a_name_read_once_routes_without_a_request_for_the_window`, `a_miss_is_never_cached`, `a_window_of_zero_reads_through_every_time`.
- AC-2: The store tells its listener every applied create, update, alias change and delete, and no refused one. This node's own change routes at once, on each backend, while another node's routes within the window. A read racing a local change is not kept. A name found gone is forgotten. Tests: `every_applied_change_is_told_and_no_other`, `this_nodes_own_changes_route_at_once`, `another_nodes_change_routes_within_the_window`, `a_read_racing_a_local_change_is_not_kept`, `a_name_found_gone_is_forgotten`.
- AC-3: Bounds:
  - the cache holds its byte budget;
  - a register past it is not kept;
  - expired entries are dropped only when a read does not fit, or past the name threshold at most once a window.

  Tests: `the_cache_holds_its_byte_budget`, `expired_names_stay_until_they_are_in_the_way`, `past_the_threshold_expired_names_are_dropped_once_a_window`, `a_prune_comes_a_full_window_after_the_last`, `a_default_cache_is_a_new_one`.
- AC-4: An alias still being created routes as absent and is not kept, and a published one routes from the cache. A malformed or oversized register is refused, not kept. Tests: `an_alias_being_created_routes_as_absent_and_is_not_kept`, `a_malformed_or_oversized_register_is_refused_not_kept`.

## Test plan

`crates/meta/tests/descriptors/routing.rs`, with exact meter counts. The race is planted through the test store's hook, which runs this node's change during the cache's read.

## S3 request budget

| Operation | PUT-class | GET-class | LIST | DELETE | Depth |
|---|---|---|---|---|---|
| `resolve`, within the window | 0 | 0 | 0 | 0 | 0 |
| `resolve`, a miss or past the window | 0 | 1 | 0 | 0 | 1 |
| `forget` | 0 | 0 | 0 | 0 | 0 |

## Switch

`RoutingCache::set_window_millis`, one second by default as in the Java; 0 reads through. The default trades a second of staleness from other nodes for no request on a warm route. A stale route is caught where it lands, by the shard's uuid check. Nothing calls it yet.

## Risks

- A hit parses the register each time: CPU for a large mapping on every request, where the Java parses once. It can be measured once a request path uses it, and the parsed form kept if it shows.
