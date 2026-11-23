# The node runs the orphan-shard sweep

## Serves

R8.4, [ADR 0028](../../adr/0028-orphan-shard-sweep.md) and [janitor-orphan-sweep](../janitor-orphan-sweep/SPEC.md): `Sweeper::slice` judges a slice of shard containers, and nothing in a node calls it. This is the timer that does, off unless asked, on the one node the rendezvous names.

## Change

- `oss_node::writer::OrphanTimer::spawn(parts, live)`: a thread that owns one `Sweeper` (it must outlive a run, for the skip-after-three-failures count) and, once every `every`, runs `slice(slice_budget)` when this node is the sweeper of the live members. `live` is a `LiveLeases` cell the node's lease thread refreshes with each membership refresh; it costs no request. A slice that needs memory reserves it from the request-body pool first; a pool without room skips the tick.
- The sweeper is the live ingest member that ranks first by rendezvous on the fixed name `orphan-sweep`, as the tombstone sweep does; every other node makes no request.
- `spawn` returns nothing, and starts no thread, for a budget of 0.
- The node reads `serverless.janitor.orphan_slice_budget` (default 0, off) and `serverless.janitor.orphan_slice_interval_millis` from its environment as `OSS_ORPHAN_SLICE_BUDGET` and `OSS_ORPHAN_SLICE_MS`, parsed by the settings' own bounds, once at boot; the sweep honours the janitor's `dry_run` as the other passes do (default off: it deletes; [janitor-default-live](../janitor-default-live/SPEC.md)). The janitor crate's settings are listed in the node's registry.

Not here: reading the settings live (the node's configuration is read at boot); the Rust snapshot writer (R14.2).

## Acceptance criteria

- AC-1: a budget of 0 starts no thread and makes no request.
- AC-2: the node the rendezvous names, with a budget above 0 and dry run off, deletes the container of an index that is gone and leaves one of a live index.
- AC-3: a node the rendezvous does not name makes no request.
- AC-4: with dry run on, the sweeper claims the cursor and counts but deletes nothing.
- AC-5: a pool without room for a slice makes no request.
- AC-6: the janitor's settings are in the node's registry with their keys and defaults, and the node's configuration reads the two environment keys within the settings' bounds and refuses a value outside them.

## Tests

`crates/node/tests/roles/orphan_timer.rs` on the emulator with its meter; `crates/node/src/boot/config.rs` tests for the keys; `the_nodes_settings_form_a_valid_registry` for the registry.

## S3 request budget

0 for a node that is not the sweeper or has the sweep off; for the sweeper, the slice's table in [ADR 0028](../../adr/0028-orphan-shard-sweep.md) once an interval (default an hour). No LIST on a request path.

## Switch

`serverless.janitor.orphan_slice_budget`, default 0: the sweep deletes data that the descriptors say is orphaned, and has not run against a Java-written deployment.

## Risks

- A node that cannot tell it is the sweeper (a stale live set) can run a slice beside another; the cursor's compare-and-swap makes the second stop (AC-6 of the sweep).
