# oss-node

The node binary. Wires the roles together; each role is compiled in only when its Cargo feature (build switch) is on.

## Owns
Process start-up, configuration loading, role selection. No domain logic: `src/main.rs` is wiring only, and the library (`src/lib.rs`) holds what is tested.

## Depends on
Every role crate, each optional behind its feature: `reader` (default), `writer`, `coordinator`.

## Entry points
`cargo run -p oss-node`, or `cargo run -p oss-node --features writer,coordinator`.
