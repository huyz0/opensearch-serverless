# Large-document soak

## Serves

Roadmap R13.5 and the bounded-memory rule ([ADR 0004](../../adr/0004-bounded-memory-and-streaming.md)): a node taking a bulk of about 100 MB of large documents holds them within its memory budget and answers every item, with no 5xx.

## Change

A test in its own binary (`crates/node/tests/large_doc_soak.rs`, the counting allocator is process-wide): a full node on the filesystem store at the smallest memory it starts with (512 MiB), one `_bulk` of 24 documents of 4 MiB each (about 96 MiB, under `http.max_content_length`), then a refresh and a count. It records the process's heap peak. No production code changes.

## Acceptance criteria

- AC-1: the bulk is answered 200 with every item created, or 429 (a refusal of the whole request for want of memory); never a 5xx, and no item fails with a server error.
- AC-2: when the bulk is taken, refreshing and counting, repeated, sees all 24 documents within 30 s.
- AC-3: the heap peak over the whole bulk stays under the node's memory budget (512 MiB; provisional: emulator-grade fs store, one machine, debug build), and under the bytes sent plus a fixed allowance, so the body is not held more than a small number of times.

## Observed, and not explained here

Run in a debug build on one machine (provisional): the heap peak was 51 to 53 MB for 96 MiB sent, under half the bytes. The first count after the first `_refresh` returned 22 of 24 documents, in two runs out of two, and all 24 were seen about 1.6 s later. The cause was not the refresh wait: a large tail fetch answered slowly because wire strings were encoded one byte at a time, fixed in [refresh-large-tail](../refresh-large-tail/SPEC.md). The test records the first count and the time to all 24.

## Test plan

`crates/node/tests/large_doc_soak.rs`.

## S3 request budget

Unchanged: no production code changes.

## Switch

None: a test only.
