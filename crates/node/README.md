# oss-node

The node binary. Wires the roles together; each role is compiled in only when its Cargo feature (build switch) is on.

## Owns
Process start-up, configuration loading, role selection. No domain logic: `src/main.rs` is wiring only, and the library (`src/lib.rs`) holds what is tested.

## Depends on
Every role crate, each optional behind its feature: `reader` (default), `writer`, `coordinator`; and `oss-types` for the settings registry.

## Entry points
`cargo run -p oss-node`, or `cargo run -p oss-node --features writer,coordinator`.

`settings::registry()` collects the settings of every crate the binary is built with and checks them ([docs/specs/settings-registry/SPEC.md](../../docs/specs/settings-registry/SPEC.md)).

With `reader`, `search::start_reader` serves `GET|POST /{index}/_search` over the indexes under a deployment base, through `search::SearchService`, which opens an index on its first search and keeps it open. Contract: [docs/specs/reader-search/SPEC.md](../../docs/specs/reader-search/SPEC.md).
