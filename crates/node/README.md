# oss-node

The node binary. Wires the roles together; each role is compiled in only when its Cargo feature (build switch) is on.

## Owns
Process start-up, configuration loading, role selection. No domain logic: `src/main.rs` is wiring only, and the library (`src/lib.rs`) holds what is tested.

## Depends on
Every role crate, each optional behind its feature: `reader` (default), `writer`, `coordinator`; and `oss-types` for the settings registry.

## Entry points
`cargo run -p oss-node`, or `cargo run -p oss-node --features writer,coordinator`.

`settings::registry()` collects the settings of every crate the binary is built with and checks them ([docs/specs/settings-registry/SPEC.md](../../docs/specs/settings-registry/SPEC.md)).

With `reader`, `search::start_reader` serves `GET|POST /{index}/_search` over the indexes under a deployment base, through `search::SearchService`, which opens an index on its first search and keeps it open, charged to the `open_indexes` pool, closing the least recently searched when the pool is full (`open_indexes`, [reader-budgets](../../docs/specs/reader-budgets/SPEC.md)). Contract: [docs/specs/reader-search/SPEC.md](../../docs/specs/reader-search/SPEC.md).

With `writer`, `writer::WriterRole` holds the shards this node writes and applies a write to the engine, stages its record in the node's one log through the committer, and acknowledges only once the blob landed; a failed stage releases the shard. `writer::RoleTarget` is its `bulk::Target`; a shard held elsewhere goes through `writer::Routes::forward`. Contract: [docs/specs/node-writer/SPEC.md](../../docs/specs/node-writer/SPEC.md).

With `writer`, `forward::BulkForwarder` sends a shard's bulk to its owner, signed, and is the one call site of `EarlyRevoke::on_refused` for a refused connect (`serverless.lease.early_revoke`, off by default). Contract: [docs/specs/forward/SPEC.md](../../docs/specs/forward/SPEC.md).

With `writer`, `WriterRole::staged` remembers the manifest a shard's last publish produced and `newest::NewestHandler` answers a reader's `internal:serverless/commit/newest` ask with it, from memory. Contract: [docs/specs/writer-newest/SPEC.md](../../docs/specs/writer-newest/SPEC.md).

With `reader`, `newest_ask::TransportNewestAsk` is a `NewestSource` a cold open asks for the newer commit a staging writer holds (`SearchService::set_newest_source`; setting `serverless.reader.newest_commit_ask`, off by default; `newest_ask::install`). Contract: [docs/specs/reader-newest-ask/SPEC.md](../../docs/specs/reader-newest-ask/SPEC.md).
