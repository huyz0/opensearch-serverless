# Object-store request budgets

What the key paths cost in object-store requests and in sequential round trips, pinned so that a change which adds one fails a test. Ported from the Java shell's `serverless/request-budgets.md`; contract: [docs/specs/request-budgets](specs/request-budgets/SPEC.md). The gates read this table through `oss_testkit::budget`, compiled into each test binary; no number here is repeated in a test.

## The rules

- **Every row is `enforced` or `blocked`.** An enforced row names the test that checks it, `crates/<crate>/tests/<path>.rs::<fn>`; a blocked row names what blocks it. There is no third status: a third status is how "we aim for zero" gets written by someone who measured nothing. `scripts/check-budgets.py` checks this, and that each enforced row's test exists and names the row's key.
- **`exact` rows ratchet.** A measurement above the budget is a regression; below it, the gate fails too and asks for the budget to be lowered, so a saving is recorded here and cannot be lost silently. Raising a budget is allowed when the new requests are worth paying: change the row in the same pull request and say why there.
- **`max` rows are ceilings,** for measurements that vary between runs by a known amount. A round-trip row whose path is wholly sequential is `exact`, so a saving in depth is recorded too.
- **Requests are counted by the store's meter, on the in-process emulator,** with its register semantics: a register swap or conditional delete is one request. On S3 that holds only when the same process just read the generation it swaps from; otherwise the S3 client reads first, a second request and a second round. An existence check is a HEAD, a LIST one request a page.
- **Round trips** are the depth of the chain a caller waits through (`oss_blob::Rounds`): calls sent together share a round, a call sent after another's answer is the next one. Each call is held 20 ms so that calls sent together overlap. A call to the emulator is one request, so one round.

To see where a count's requests go, record instead of checking; each gate then prints its measurement and the calls it made, by kind and key:

```bash
OSS_REQUEST_BUDGETS_RECORD=1 cargo test -p oss-node --test it budgets -- --nocapture
```

## Budgets

| Objective | Case | Budget | Bound | Status | Gate |
|---|---|---|---|---|---|
| `search.cold.requests` | A reader node opens an index it never served and runs one term search: the descriptor, the commit, and the segment's opening files in one merged ranged read | 3 | exact | enforced | `crates/node/tests/it/budgets.rs::a_cold_search_then_a_warm_one` |
| `search.cold.round_trips` | Round trips that open and first search wait through: the descriptor, the commit, then the opening files sent together (open prefetch, [spec](specs/reader-open-prefetch/SPEC.md)) | 3 | exact | enforced | `crates/node/tests/it/budgets.rs::a_cold_search_then_a_warm_one` |
| `search.warm.requests` | The next search on that index | 0 | exact | enforced | `crates/node/tests/it/budgets.rs::a_cold_search_then_a_warm_one` |
| `search.metric_aggs.cold.requests` | The cold term search with a `stats`, a `max`, a `terms`, a `histogram`, a `range` and a `filters` aggregation, each with a metric sub-aggregation: the doc values come through the same ranged reads as the hits, so no request is added | 3 | exact | enforced | `crates/node/tests/it/aggs_budgets.rs::a_cold_search_with_metric_aggregations_then_a_warm_one` |
| `search.metric_aggs.cold.round_trips` | Its round trips | 3 | exact | enforced | `crates/node/tests/it/aggs_budgets.rs::a_cold_search_with_metric_aggregations_then_a_warm_one` |
| `search.metric_aggs.warm.requests` | The next aggregating search on that index | 0 | exact | enforced | `crates/node/tests/it/aggs_budgets.rs::a_cold_search_with_metric_aggregations_then_a_warm_one` |
| `search.keyword_terms.cold.requests` | The cold term search with a `stats` and a keyword `terms` with a `max` sub-aggregation: the dictionary, ordinals and sub-aggregation doc values come through the same ranged reads as the hits, so no request is added, whatever the sub-aggregations | 3 | exact | enforced | `crates/node/tests/it/aggs_budgets.rs::a_cold_search_with_a_keyword_terms_then_a_warm_one` |
| `search.keyword_terms.cold.round_trips` | Its round trips | 3 | exact | enforced | `crates/node/tests/it/aggs_budgets.rs::a_cold_search_with_a_keyword_terms_then_a_warm_one` |
| `search.keyword_terms.warm.requests` | The next such search on that index | 0 | exact | enforced | `crates/node/tests/it/aggs_budgets.rs::a_cold_search_with_a_keyword_terms_then_a_warm_one` |
| `descriptor.create.requests` | Creating an index's descriptor: one conditional create | 1 | exact | enforced | `crates/meta/tests/descriptors/budgets.rs::descriptor_operations_cost_what_the_table_says` |
| `descriptor.read.requests` | Reading a descriptor | 1 | exact | enforced | `crates/meta/tests/descriptors/budgets.rs::descriptor_operations_cost_what_the_table_says` |
| `descriptor.update.requests` | Swapping a descriptor whose generation was just read | 1 | exact | enforced | `crates/meta/tests/descriptors/budgets.rs::descriptor_operations_cost_what_the_table_says` |
| `descriptor.delete.requests` | Deleting a descriptor if unchanged (conditional deletes) | 1 | exact | enforced | `crates/meta/tests/descriptors/budgets.rs::descriptor_operations_cost_what_the_table_says` |
| `index.create.requests` | Creating an index through the lifecycle | 1 | exact | enforced | `crates/meta/tests/descriptors/budgets.rs::an_index_create_and_delete_cost_what_the_table_says` |
| `index.delete.requests` | Deleting a two-shard index no writer holds, with shard heads and snapshot pins stubbed (their blob reads are not ported): the read, the reclaim intent, the conditional delete, then a LIST and a delete a shard | 7 | exact | enforced | `crates/meta/tests/descriptors/budgets.rs::an_index_create_and_delete_cost_what_the_table_says` |
| `index.delete.round_trips` | Round trips that delete waits through: the shards are purged one after another | 7 | exact | enforced | `crates/meta/tests/descriptors/budgets.rs::an_index_create_and_delete_cost_what_the_table_says` |
| `alias.update.requests` | Moving an alias from members a, b to b, c: the reservation, then each changed member's hint, then the publish | 8 | exact | enforced | `crates/meta/tests/descriptors/budgets.rs::an_alias_update_costs_what_the_table_says` |
| `alias.update.round_trips` | Round trips that update waits through | 8 | exact | enforced | `crates/meta/tests/descriptors/budgets.rs::an_alias_update_costs_what_the_table_says` |
| `cluster_settings.first_update.requests` | The first persistent settings update: a read finding none, then a create | 2 | exact | enforced | `crates/meta/tests/descriptors/budgets.rs::a_cluster_settings_update_costs_what_the_table_says` |
| `cluster_settings.update.requests` | A later update: a read, then a swap | 2 | exact | enforced | `crates/meta/tests/descriptors/budgets.rs::a_cluster_settings_update_costs_what_the_table_says` |
| `cluster_settings.update.round_trips` | Round trips that update waits through | 2 | exact | enforced | `crates/meta/tests/descriptors/budgets.rs::a_cluster_settings_update_costs_what_the_table_says` |
| `take.fresh.requests` | First take of four never-written shards | 50 | max | blocked | The writer's shard take is not ported |
| `write.first.requests` | The node's first write after the take: opens its log epoch | 4 | exact | enforced | `crates/node/tests/writer_first.rs::the_first_write_after_a_take_costs_what_the_table_says` |
| `write.next.requests` | The next write to the same shard, inside the descriptor fence window: one log PUT | 1 | exact | enforced | `crates/writer/tests/node_log_budgets.rs::the_next_write_to_a_shard_is_one_put_in_one_round` |
| `publish.requests` | One write to each of four shards, then a publish of all four | 23 | exact | blocked | Segment publish is not ported |
| `write.first.round_trips` | Round trips the node's first write waits through (Java: 5, a ceiling) | 5 | max | enforced | `crates/node/tests/writer_first.rs::the_first_write_after_a_take_costs_what_the_table_says` |
| `write.next.round_trips` | Round trips the next write waits through (Java: 1, a ceiling) | 1 | max | enforced | `crates/writer/tests/node_log_budgets.rs::the_next_write_to_a_shard_is_one_put_in_one_round` |
| `lease.first_renewal.requests` | A node's first renewal: its lease created, the members index read, the index swapped to name it | 3 | exact | enforced | `crates/cluster/tests/leases/budget.rs::lease_operations_cost_what_the_table_says` |
| `lease.renewal.requests` | Every later renewal: one swap of its own lease | 1 | exact | enforced | `crates/cluster/tests/leases/budget.rs::lease_operations_cost_what_the_table_says` |
| `lease.refresh.requests` | A refresh while every held lease copy has half a lifetime left: the members index | 1 | exact | enforced | `crates/cluster/tests/leases/budget.rs::lease_operations_cost_what_the_table_says` |
| `lease.refresh_digest.requests` | A refresh once the copies ran out: the index and the lease digest, no lease | 2 | exact | enforced | `crates/cluster/tests/leases/budget.rs::lease_operations_cost_what_the_table_says` |
| `lease.revoke.requests` | Revoking a dead node's lease: a read and a swap | 2 | exact | enforced | `crates/cluster/tests/leases/budget.rs::lease_operations_cost_what_the_table_says` |
| `lease.release.requests` | A clean release: the index and the lease read, the lease and the index swapped | 4 | exact | enforced | `crates/cluster/tests/leases/budget.rs::lease_operations_cost_what_the_table_says` |
| `lease.idle.writer_swaps_per_hour` | An idle hour of three nodes renewing and refreshing every 10 s: the digest writer's swaps, a renewal and a digest write each time (Java: 720) | 720 | exact | enforced | `crates/cluster/tests/leases/budget.rs::an_idle_hour_costs_what_the_table_says` |
| `lease.idle.writer_gets_per_hour` | The same hour: the digest writer's reads, the index, the digest and every lease each time | 1800 | exact | enforced | `crates/cluster/tests/leases/budget.rs::an_idle_hour_costs_what_the_table_says` |
| `lease.idle.reader_swaps_per_hour` | The same hour: another node's swaps, its renewals | 360 | exact | enforced | `crates/cluster/tests/leases/budget.rs::an_idle_hour_costs_what_the_table_says` |
| `lease.idle.reader_gets_per_hour` | The same hour: another node's reads, the index each time and the digest when its copies run out | 719 | exact | enforced | `crates/cluster/tests/leases/budget.rs::an_idle_hour_costs_what_the_table_says` |
| `lease.idle.lists_per_hour` | The same hour: LISTs by any node | 0 | exact | enforced | `crates/cluster/tests/leases/budget.rs::an_idle_hour_costs_what_the_table_says` |
| `held_idle.index_requests_per_hour` | An hour with four shards held and nothing written or searched: requests charged to the indices (Java: 64) | 64 | exact | blocked | The writer's shard holding is not ported, and no periodic job runs on a simulated clock |
| `held_idle.node_requests_per_hour` | The same hour: requests charged to the node, for lease renewal, the stored-script marker and sweeps (Java: 1,227) | 1227 | exact | blocked | The writer's lease and sweeps are not ported, and no periodic job runs on a simulated clock |
| `held_idle.index_requests_target` | An idle held index makes no periodic request | 0 | exact | blocked | No writer holds shards yet. The Java misses it, and a port must not copy why: head verification reads each held shard's head every 5 minutes (12 per shard-hour), the 5-minute descriptor refresh backstop reads each descriptor, and the orphan sweep reads each descriptor about 3 times an hour. Zero needs head checks carried by the lease or a change feed, and the backstop replaced by the change stripes |
| `release.requests` | Releasing four idle shards | 43 | exact | blocked | The writer's shard release is not ported |
| `take.released.requests` | Taking one shard its writer released cleanly | 25 | exact | blocked | The writer's shard take is not ported |
| `take.released.round_trips` | Round trips that take waits through (Java: 24, a ceiling) | 24 | max | blocked | The writer's shard take is not ported |
| `idle_released.node_requests_per_hour` | An hour of a writer's periodic work once four written indices were released (Java: 1,168) | 1168 | exact | blocked | The writer's periodic work is not ported, and no periodic job runs on a simulated clock |
| `search.held_idle.index_requests_target` | An idle shard a search node holds open makes no periodic request | 0 | exact | blocked | The reader has no periodic check yet; it arrives with refresh on the node runtime, and needs a simulated clock |
