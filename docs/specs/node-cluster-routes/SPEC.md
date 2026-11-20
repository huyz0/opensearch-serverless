# A node serves cluster health, cluster settings and the cat and list tables over REST

## Serves

R7.5: the routes of the shell's `ClusterHealthHandler`, `ClusterSettingsHandler` and `ListIndicesHandler`, over the decisions [coordinator-cluster-health](../coordinator-cluster-health/SPEC.md), [coordinator-cluster-settings](../coordinator-cluster-settings/SPEC.md) and [coordinator-cat-tables](../coordinator-cat-tables/SPEC.md) already make. This change reads the metadata plane and draws the answers; the judging and the words are those specs'.

## Change

`oss-node` (the `writer` feature, which now also holds `oss-coordinator`) gains:

- `cluster_route`: `ClusterService` and `settings_routes`. `GET /_cluster/settings` reads the cluster config register; `PUT /_cluster/settings` checks the body with `parse_update` and writes it with `ClusterConfig::update`.
- `health_route`: `HealthSource` (the live node count and what an index's heads say), `StoreHealth` (that, over `Membership`, `ShardHeadStore` and `DescriptorStore`) and `routes`: `GET /_cluster/health`, `/_cluster/health/{index}` and `/_cat/health`.
- `cat_route`: `CatService` and `routes`: `GET /_cat/indices[/{index}]` and `GET /_list/indices[/{index}]`.
- `assemble` registers the settings and the cat routes on every node, and `Assembled::with_health` registers the health routes once the caller has a `HealthSource` (a node that has enrolled has a `Membership`; the assembly does not make one).

Not here: applying a settings write to this node's live settings (the refresh timer, with the node runtime), the plugins' settings check (the closure is `Ok`), `include_defaults` content, and `docs.count` and `store.size` columns.

## Acceptance criteria

- AC-1: `GET /_cluster/settings` answers the register's persistent settings nested by their dotted names (flat with `flat_settings=true`), an empty `transient` and, with `include_defaults`, an empty `defaults`; a register never written reads as empty settings.
- AC-2: `PUT /_cluster/settings` writes the persistent settings and answers `acknowledged`, the settings now in the register and an empty `transient`; a null removes a key; a second update merges with the first.
- AC-3: a refused update (no body, no `persistent`, a transient setting, a list, a bad value of a checked key, a checkpoint age the commit period forbids) answers the coordinator's status, type and words and writes nothing.
- AC-4: `GET /_cluster/health` answers `status`, `status_scope: control_plane`, `number_of_nodes`, `number_of_data_nodes`, `timed_out`, `complete: false` and no shard counters; `/_cluster/health/{index}` answers the index's counts, `dormant_shards`, `unassigned_shards` for unservable ones, `complete: true`, and under `indices` each index's shard counts (and each shard's state with `level=shards`).
- AC-5: an index that does not exist is 404 `index_not_found` "no such index: <name>"; a source that cannot read the node count answers red; an index whose head cannot be read is red and its shard `unservable`.
- AC-6: `wait_for_nodes` waits (polling every 200 ms) until its condition holds or `timeout` (default 30s) passes; a wait that ran out answers 408 with `timed_out: true`, a satisfied one 200; a bad condition is 400 `invalid_parameter`; `wait_for_status` without an index is 400 `unscoped_wait_for_status`.
- AC-7: the refusals `refuse` words (`wait_for_status`, `wait_for_active_shards`, `wait_for_events`, `level`) answer 501 `unsupported_health_parameter`; the hints the shell reads and ignores are accepted.
- AC-8: `GET /_cat/health` answers the one-row table (text, with the header under `v`, or JSON under `format=json`), its `shards` and `dormant` cells `-` always, the cat route naming no index.
- AC-9: `GET /_cat/indices` and `/_cat/indices/{index}` answer one row for each index the names resolve to (a name, a comma list, a prefix pattern; the bare form is `*`), `size` and `next_token` are refused, a pattern past the cap is 400 `too_many_indices`, an absent named index is 404, and a pattern matching nothing is an empty table.
- AC-10: `GET /_list/indices[/{index}]` answers pages `{"next_token":..,"indices":[..]}` of `size` rows (default 500) in name order, the token resuming the walk, and the refusals `plan` words for a bad `size`, a token on a non-prefix expression and a tainted token; named indices answer one page with a null token.
- AC-11: `h` and `s` are 501 `unsupported_cat_parameter`.
- AC-12: requests are exact: a settings read is one GET of the register, an update one GET and one PUT, `/_cluster/health/{index}` one GET of the descriptor and one of each head, an unscoped health none, and a cat of named indices one GET each.

## Tests

`crates/node/tests/cluster_routes.rs` over the emulator and a real server socket, the health source a fake for the cases the store cannot produce.

## S3 request budget

As AC-12: nothing scales with the deployment. A pattern costs one capped LIST (`names_with_prefix`) plus a GET a name; a `_list` page costs `list_page`'s (a LIST per 1,000 keys read plus a GET a row). The node count is `Membership::current`, which makes no request. Polling re-reads the heads each 200 ms for as long as the caller asked to wait (bounded by `timeout`).

## Bounds

A settings request holds one buffer of 6 times `MAX_CLUSTER_SETTINGS_BYTES` and its body, charged to the request-body pool; a descriptor read holds one 1 MiB buffer, likewise. A cat answer holds the rows of at most the pattern cap (1,000) or a page (5,000).

## Switch

None: the shell's routes have no setting and no measured downside.

## Risks

- The pattern cap is fixed at 1,000 here; the shell reads it from a search setting.
- No plugin owns an index here, so no name is a system index.
- A health wait occupies a worker for up to its timeout, as the shell's does a generic-pool thread.
