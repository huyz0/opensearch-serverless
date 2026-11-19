# Reader: open prefetch

## Serves

Parity task R10.8 (read prefetch), the open half; the request budget's round-trip depth (`search.cold.round_trips_target`: the descriptor, the commit, then the opening ranges sent together); [ADR 0021](../../adr/0021-open-prefetch-gate.md) (the prefetch's gate, which sits beside ADR 0005's executor and ADR 0004's pools).

## Change

A cold shard's open reads each small or pinned file of its commit whole, one after another: the commit file, then each segment's `.si`, then its field infos, metadata and term-index files. Before the engine opens the commit, the reader now plans those files from the commit's file table (names and lengths, already known: no LIST, no HEAD), merges the ones that sit near each other in one blob into a single ranged GET, fetches the merged ranges together, and serves the engine's reads of those files from the fetched bytes. The bytes are dropped when the open returns; the page cache keeps what it kept before.

A file is read whole by the engine when its name is pinned (`PageCache::is_pinned_name`) or it fits one page; the plan picks exactly those. Large files (postings, doc values, stored fields) are untouched: queries still announce those ranges themselves. A failed prefetch is a missed hint: the engine reads the file itself, as it did.

Not changed: the descriptor and commit register reads, the search path, and any file the plan does not pick.

## Acceptance criteria

- AC-1: a cold open and first term search of a one-segment shard makes 3 round trips (descriptor, commit, one merged read of the segment's opening files) and 3 requests; with the prefetch off it makes 6 and 6.
- AC-2: for a shard of several segments in several blobs, the prefetch makes no more requests than the same open with it off, and a depth of 3.
- AC-3: the plan merges files of one blob whose gap is at most `gap_bytes`, never merges across blobs, never picks a file the engine would page (large, unpinned), and never reads more than `max_bytes`, gaps included.
- AC-4: a prefetch that fails (the store refuses a range) leaves the open succeeding with the engine's own reads, and the search answer unchanged.
- AC-5: after the open returns the fetched bytes are no longer held and the gate holds nothing.
- AC-6: the search answers of every compat replay step are unchanged, and `docs/request-budgets.md` rows `search.cold.*` and `search.metric_aggs.cold.*` are the new, lower numbers, enforced.
- AC-8: over the layout the Java shell writes (a segment is one blob), a cold open of each of the seven compat indexes makes fewer requests, the same or fewer round trips and the same bytes with the prefetch on as off.
- AC-7: however many opens run at once, the bytes and fetches held never pass the gate, a refused range leaves its files to the engine, and everything taken is given back.

## Test plan

- AC-1, AC-2, AC-4, AC-5: `prefetch::tests` in `oss-reader`, over the emulator behind `Rounds`; seen red against the unchanged open.
- AC-3: `prefetch::plan` unit tests with a hand-built file table.
- AC-7: `prefetch::gate_tests` and the gate tests in `prefetch::tests`, with several opens at once on a small gate.
- AC-8: `crates/node/tests/it/open_prefetch.rs`.
- AC-6: `crates/node/tests/it/budgets.rs`, `aggs_budgets.rs`, and the compat replay.

## S3 request budget

Never more requests than the engine's own reads of the same files: a merged range replaces the ranges of its files, and the plan adds none. Measured (emulator, provisional): over the shell's layout the seven compat indexes open in 3 requests, or 4, where they took 6, or 9, with the same bytes (`the_shell_s_layout_opens_in_fewer_requests_and_the_same_bytes`); the testkit's one-blob-per-file indexes of one, four and a thousand segments read the same requests and bytes (`a_cold_open_reads_the_same_requests_and_bytes_with_the_prefetch_as_without`, which cannot show a merge). Bytes can rise by the gaps it spans (at most `gap_bytes` each, default 32 KiB) and by small files the open would not have read (at most `max_bytes` in all, default 8 MiB, gaps included); a segment written by the shell is one blob, so its opening files are usually one range. Depth falls from 2 + the files read one after another to 3. LIST 0, HEAD 0, PUT 0. Idle: 0.

## Memory

At most `max_bytes` (8 MiB) read for one open, the gaps between files included (a range that would pass it is left to the engine), and at most the node-wide `Gate` (32 MiB and 32 fetches, [ADR 0021](../../adr/0021-open-prefetch-gate.md)) for every open of the node together. Each range takes its bytes and a thread from the gate before it allocates or spawns; a refusal leaves the range to the engine. The bytes are a second copy beside the engine's own copy of the same small files, which [`open_cost`](../../../crates/reader/src/open_cost.rs) charges once. Freed when the open returns.

## Switch

`OpenPrefetch { enabled, gap_bytes, max_bytes }`; on by default, because the requests never rise and depth falls (measured above, provisional: emulator). The downside is bytes, bounded above. Wiring it to a cluster and per-index setting, and the gate to the node's memory budget, are the node's, which this change does not touch; `open_located_with` takes the switch and its gate.

## Risks

A shard whose small files the open does not read pays their bytes: the cap bounds it. A refused range of a merged read hides its files from the prefetch only: AC-4.
