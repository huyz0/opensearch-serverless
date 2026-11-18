# Writer soak: a thousand shards publishing

## Serves

Parity plan R6.6, the writer's request cost and heap at a thousand shards.

## Change

A test in its own binary (`crates/writer/tests/writer_soak.rs`): 1,000 shards, each in its own directory with its own publisher over one metered emulator, take a document, refresh and publish for three rounds. It counts the requests and the peak heap. No production code changes.

Not changed, by design and listed: write p99 against the Java writer (the ack meter in `node_log_ack_meter` and `node_log_open_load` holds the Rust side; the Java side is a recorded number), the node writer role running the loop, indexing-memory driven refreshes.

## Acceptance criteria

- AC-1: every publish makes one blob PUT and one register PUT and no LIST, so PUT plus LIST per publish is 2.0, within the Java writer's 2.09.
- AC-2: the peak heap of 1,000 open shards and their directories stays under 128 MiB (provisional).

## Test plan

`crates/writer/tests/writer_soak.rs`, counting allocator and the emulator's meter.

## S3 request budget

Asserted: 2 PUT, 0 LIST a publish, 6,000 PUT for 3,000 publishes.

## Switch

None: a test only.

## Risks

- Heap is measured in a debug build on one machine; it is a bound against growth, not a size to quote.
