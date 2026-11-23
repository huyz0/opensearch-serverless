# oss-node

The node binary. Wires the roles together; each role is compiled in only when its Cargo feature (build switch) is on.

## Owns
Process start-up, configuration loading, role selection. No domain logic: `src/main.rs` is wiring only, and the library (`src/lib.rs`) holds what is tested.

## Depends on
Every role crate, each optional behind its feature: `reader` (default), `writer`, `coordinator`; and `oss-types` for the settings registry.

## Entry points
`cargo run -p oss-node`, or `cargo run -p oss-node --features writer,coordinator`.

`settings::registry()` collects the settings of every crate the binary is built with and checks them ([docs/specs/settings-registry/SPEC.md](../../docs/specs/settings-registry/SPEC.md)).

With `reader`, `search::start_reader` serves `GET|POST /{index}/_search` over the indexes under a deployment base, through `search::SearchService`, which opens an index on its first search and keeps it open, charged to the `open_indexes` pool, closing the least recently searched when the pool is full (`open_indexes`, [reader-budgets](../../docs/specs/reader-budgets/SPEC.md)). Contract: [docs/specs/reader-search/SPEC.md](../../docs/specs/reader-search/SPEC.md). `search_count` adds `GET|POST /{index}/_count` as the exact total of a size-0 search ([docs/specs/count-api/SPEC.md](../../docs/specs/count-api/SPEC.md)). `search_field_caps` adds `GET|POST /{index}/_field_caps?fields=` over named indices ([docs/specs/field-caps-api/SPEC.md](../../docs/specs/field-caps-api/SPEC.md)). `search_msearch` adds `GET|POST [/{index}]/_msearch`, running each search of the body as its line ends ([docs/specs/msearch-api/SPEC.md](../../docs/specs/msearch-api/SPEC.md)). `search_validate` adds `GET|POST [/{index}]/_validate/query`, building the query over shard 0's mapping without a search ([docs/specs/validate-query-api/SPEC.md](../../docs/specs/validate-query-api/SPEC.md)). `search_template` adds `GET|POST /_render/template` and `GET|POST [/{index}]/_search/template`, rendering an inline mustache template with `oss_reader::template` and searching the result ([docs/specs/search-template-api/SPEC.md](../../docs/specs/search-template-api/SPEC.md)). `search_analyze` adds `GET|POST [/{index}]/_analyze` over the `standard` and `keyword` analyzers ([docs/specs/analyze-api/SPEC.md](../../docs/specs/analyze-api/SPEC.md)).

With `writer`, `writer::WriterRole` holds the shards this node writes and applies a write to the engine, stages its record in the node's one log through the committer, and acknowledges only once the blob landed; a failed stage releases the shard. `writer::RoleTarget` is its `bulk::Target`; a shard held elsewhere goes through `writer::Routes::forward`. Contract: [docs/specs/node-writer/SPEC.md](../../docs/specs/node-writer/SPEC.md).

With `writer`, `forward::BulkForwarder` sends a shard's bulk to its owner, signed, and is the one call site of `EarlyRevoke::on_refused` for a refused connect (`serverless.lease.early_revoke`, off by default). Contract: [docs/specs/forward/SPEC.md](../../docs/specs/forward/SPEC.md).

With `writer`, `bulk_handler::BulkHandler` is what an owner runs for a forwarded bulk: it applies the group to its shard with `WriterRole::apply_group` and answers each item's outcome. Contract: [docs/specs/node-bulk-handler/SPEC.md](../../docs/specs/node-bulk-handler/SPEC.md).

With `writer`, `cluster_owner::ClusterRoutes` is the cluster half of `writer::Routes`: the owner of a shard by rendezvous over the held leases, and a group forwarded as one bulk through `BulkSend`. Contract: [docs/specs/node-cluster-routing/SPEC.md](../../docs/specs/node-cluster-routing/SPEC.md).

With `writer`, `WriterRole::staged` remembers the manifest a shard's last publish produced and `newest::NewestHandler` answers a reader's `internal:serverless/commit/newest` ask with it, from memory. Contract: [docs/specs/writer-newest/SPEC.md](../../docs/specs/writer-newest/SPEC.md).

With `reader`, `newest_ask::TransportNewestAsk` is a `NewestSource` a cold open asks for the newer commit a staging writer holds (`SearchService::set_newest_source`; setting `serverless.reader.newest_commit_ask`, off by default; `newest_ask::install`). Contract: [docs/specs/reader-newest-ask/SPEC.md](../../docs/specs/reader-newest-ask/SPEC.md).

With `reader`, `tail_overlay::install` installs an `OverlaySource` a search asks for the overlay of the shard it serves (`SearchService::set_overlay_source`; setting `serverless.search.tail_overlay.enabled`, off by default). Contract: [docs/specs/reader-overlay-source/SPEC.md](../../docs/specs/reader-overlay-source/SPEC.md). `overlay_builder::OverlayBuilder` is the source over `TailHeld`: it builds on a worker thread, never in a search, and charges the `open_indexes` pool without evicting ([docs/specs/node-overlay-builder/SPEC.md](../../docs/specs/node-overlay-builder/SPEC.md)). `OverlayBuilder::fetching` also asks a shard's writer once per stage for the records past the commit ([docs/specs/node-overlay-fetch/SPEC.md](../../docs/specs/node-overlay-fetch/SPEC.md)). `tail_overlay::wire` turns all of it on for a node under the setting and hands back the push handler to register ([docs/specs/node-overlay-wiring/SPEC.md](../../docs/specs/node-overlay-wiring/SPEC.md)).

With `writer`, `tail_keep::TailKeep` holds the writes a node acknowledged and has not published (20,000 records and 16 MiB a shard, 256 MiB a node) for a reader opening an older commit. Nothing calls it yet. Contract: [docs/specs/writer-tail-keep/SPEC.md](../../docs/specs/writer-tail-keep/SPEC.md).

`tail_serve::TailServe` answers a reader's tail fetch from it (records past the reader's commit, a subscription with a 600 ms grant, the claim); nothing registers it as a handler yet. Contract: [docs/specs/writer-tail-fetch/SPEC.md](../../docs/specs/writer-tail-fetch/SPEC.md).

`tail_push::TailPush` queues the writes a node acknowledged and, on `flush`, sends each subscribed reader one `PushRequest` and counts its answer through `TailServe`; the caller schedules the flush and nothing calls it yet. Contract: [docs/specs/writer-tail-push/SPEC.md](../../docs/specs/writer-tail-push/SPEC.md).

With `reader`, `tail_held::TailHeld` holds the writes a writer pushed a reader (100,000 records a shard) and the grant its fetch answered; nothing applies them yet. Contract: [docs/specs/reader-tail-held/SPEC.md](../../docs/specs/reader-tail-held/SPEC.md).

The tail's transport (docs/specs/node-tail-links/SPEC.md): with `writer`, `tail_link::TransportTailLink` sends a tail push to a reader a lease names and `TailFetchHandler` answers a fetch from `TailServe`; with `reader`, `tail_reader_link::TailPushHandler` gives a push to `TailHeld` and `TransportTailFetch` asks a live writer for the records past a commit. Nothing registers the handlers yet.

With `writer`, `cluster_route`, `health_route` and `cat_route` serve `GET`/`PUT /_cluster/settings`, `GET /_cluster/health[/{index}]`, `GET /_cat/health` and `GET /_cat/indices` and `/_list/indices` over the registers and the decisions in `oss-coordinator`; the assembly registers the settings and the index tables, and `Assembled::with_health` the health routes over a `HealthSource` (`StoreHealth` reads the membership and the shard heads). Contract: [docs/specs/node-cluster-routes/SPEC.md](../../docs/specs/node-cluster-routes/SPEC.md).

`handlers::Registry` holds the transport actions a node serves, each behind the signature check, and refuses a second handler for an action; the assembly adds each role's handler to it ([docs/specs/node-handlers/SPEC.md](../../docs/specs/node-handlers/SPEC.md)).

`SearchService::open(index, shard)` opens any shard of an index, one open shard each in the `open_indexes` pool; the HTTP search still serves indexes of one shard ([docs/specs/node-multi-shard-open/SPEC.md](../../docs/specs/node-multi-shard-open/SPEC.md)).

With `reader` and `writer`, `Assembled::wire_tail` turns the tail overlay on under `serverless.search.tail_overlay.enabled` (off by default): `tail_node::wire` builds the keep, serve, push and flush timer, installs the overlay builder, sets the writer role's `TailSink` and returns the `handlers::Registry` holding the tail fetch and push actions. `tail_node::LeaseWriters` is the `LiveWriters` over the node's leases. Contract: [docs/specs/node-tail-wiring/SPEC.md](../../docs/specs/node-tail-wiring/SPEC.md).

With `reader` and `writer`, `boot::start` starts a node from `OSS_*` environment configuration (`boot::NodeConfig`): it opens the store, enrols and renews the node's lease, builds the write log over the shard heads, assembles the roles and the health routes, and listens; `main` calls it. `Node::crash` stops a node without releasing its lease, as a killed process would. It also serves the native transport and routes each write to the node that owns its shard. Contract: [docs/specs/node-boot/SPEC.md](../../docs/specs/node-boot/SPEC.md).
