# 0021: The open's prefetch fetches outside the page cache, under one node-wide gate of bytes and threads

**Status:** Accepted by the owner, 2026-11-19, after two independent reviews that did not clear it (it trades away ADR 0004's reserve-before-allocate and ADR 0005's single executor for this path). Amends [ADR 0004](0004-bounded-memory-and-streaming.md) and [ADR 0005](0005-io-model.md) for this one path until the sunset below.

## Context

A cold open reads the commit file, each segment's `.si` and the other small and pinned files one after another, because the engine announces nothing before it has read the `.si`. That is 6 requests in 6 round trips for a one-segment index, against a goal of 3 ([request budgets](../request-budgets.md)). [ADR 0005](0005-io-model.md) sends every fan-out through the engine's one bounded range-fetch executor under its page cache, and [ADR 0004](0004-bounded-memory-and-streaming.md) reserves memory from a pool before allocating. The engine's own `Directory::prefetch_files` cannot do this job: it fetches nothing without a disk tier, and it runs only after each `.si` is read. PR #159 shipped a reader-side prefetch (`crates/reader/src/prefetch/`) that fetched ranges on its own threads into plain buffers, bounded per open and not per node, and an independent review found it outside both ADRs.

## Decision

1. The reader plans the files the engine reads whole (pinned names and files of at most one page) from the commit's file table, merges neighbours of one blob, fetches the merged ranges together and serves the engine's reads of those files from the bytes until the open returns. This is a fetch layer above the page cache for the open only; queries still announce their own ranges to the engine.
2. Its threads and bytes come from one process-wide `Gate` (32 MiB and 32 fetches, defaults) taken without waiting, before any buffer is allocated or thread spawned. However many shards open at once, the prefetch holds at most the gate. A range the gate refuses is left to the engine: the prefetch is a hint, so a refusal costs round trips, never an answer or a 429.
3. A fetched range's bytes stay reserved until the open returns, then both go back.
4. The prefetch is a switch (`OpenPrefetch::enabled`), on by default, because it measured a win with no cost on the shapes tried: over the shell's layout (a segment is one blob) the seven compat indexes open in 3 requests, or 4, instead of 6, or 9, in 3 round trips instead of 6, reading the same bytes (`crates/node/tests/it/open_prefetch.rs`, emulator, provisional); the testkit's one-blob-per-file indexes read the same requests and bytes, which shows only that nothing is added there. A shell index whose small files sit apart in a segment blob would pay the gap bytes, not yet measured. It can read more only by the gaps it spans and small files the open would not read, bounded per open (8 MiB, gaps included) and per node by the gate; in a layout with one blob per file an unneeded small file would be one extra request.
5. Sunset: the exception ends on 2026-12-31, or when the node's open-path work moves the merged ranges into the engine's executor and the bytes into the node's `MemoryBudget`, whichever is first.

## Consequences

- Requests and depth of a cold open fall with no extra request; the downside is bytes, bounded twice.
- The gate is a reader-local pool, not an `oss-mem` pool, because the open takes no `MemoryBudget`; it is an explicit exception to ADR 0004's "pool before allocate", justified by a refusal being free. The node's memory can exceed its budget by the gate's 32 MiB, and 32 blocking threads run outside the executor, while it stands.
- Fetched bytes are held for the open and the page cache then keeps what it kept before: the engine's reads of those files are served from them, so nothing is fetched twice. Moving it into the node's budget, and the cluster and per-index setting for the switch, wait for the node's open path.
- The threads are outside ADR 0005's executor but counted by the gate. Handing the merged ranges to the engine's executor, once it takes ranges for files not yet opened, would replace the threads and make this ADR's second decision hold by construction.
- A cold start of many shards at once shares the 32 MiB gate: past about four full-size opens the later ones read as before, at the old depth. Not measured.
- Idle indexes make no request: the prefetch runs only inside an open.
