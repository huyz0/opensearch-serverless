# Reader: warm copy on join

## Serves

Parity task R12.1 (hot set, peer cache copy, parallel fill to 90% before ready); ADR 0049; R12.2's ready flag (PR 423: `Readiness`, `/_serverless/ready`, the lease's `search` role). Lands in three PRs, each mergeable alone: (1) the node's optional disk tier, behind lucene-rust PR 570; (2) the hot-set and page actions and the copy; (3) the `Warming` state. (3) needs PR 423.

## Change

A node with a disk tier (`OSS_PAGE_CACHE_DIR`) keeps the pages it fetched, counting their reads. A joining node with `serverless.search.warm_on_join.enabled` asks each search peer for the shards of its readers the joiner will serve and their hottest held pages, then takes those pages from the peers into its own disk tier, in batches, paced. It answers 503 on `/_serverless/ready` and holds the lease without `search` until it holds `ready_fraction` of the bytes named or `max_wait` passes.

Not changed: the search path; a node without a disk tier or with the switch off; the writer and the tail overlay.

## Acceptance criteria

- AC-1: with `OSS_PAGE_CACHE_DIR` and `OSS_PAGE_CACHE_BYTES` set a node's cache holds the pages a search fetched and counts their reads; unset, it holds what it did before (no disk tier). Both runs search the same data and answer the same.
- AC-2: a peer's hot set names exactly the shards where the joiner is among the first `width` readers of `readers_for` with the joiner added (membership and widths injected), at most `max_shards`, each shard's pages most read first, the whole under `max_bytes`; none of a frozen, unopened or index-off shard; a listing over a tier that holds a `segments` file and a pinned-extension file leaves their pages out (and the joiner drops them if sent).
- AC-3: a page batch, over pages the peer has evicted and pages it never held as well as held ones, answers only keys under a shard the peer holds as a reader (a request naming another key gets a refusal and no bytes), at most `max_pages_per_request` pages, served with `peek_page`: a page the peer no longer holds is reported missed, and the peer's object-store request count over the batch is 0, counted at its metered store; a joiner and peer with different page sizes refuse each other.
- AC-4: the joiner never has more than `parallel_peers` peers asked at once or more than its reserved batch bytes in memory; its sleeps, taken through an injected `Pause`, add up to at least `bytes / max_bytes_per_second`, and a fake clock shows no batch sent early.
- AC-5: two nodes, the peer warm over a fixed set of searches: the joiner's object-store GETs over the whole warm-up plus its first run of the same searches are fewer than a cold joiner's GETs for the same searches by at least the pages copied (the exact counts are in VERIFIED.md and in the request budget table), and both answer as the peer does. The expected GETs are computed from the page and read-ahead window sizes and written in the budget row, not from the copy count (one copied page can save a window's GET or none).
- AC-6: a fake peer that never answers, refuses the action (a Java node), answers with a short page, a wrong page size or an error mid-copy, leaves the joiner ready by `max_wait` at the latest, with the same search answers and every other peer's pages copied.
- AC-12: a peer that refuses a page batch fast (as one serving its `max_concurrent` limit does) is asked again, up to 6 times with a wait that doubles from 25 ms, and its copy goes on; a peer that refuses all 6 is given up on as before, and its remainder is dropped from the target; a call that failed after a second or more (a timeout) is not asked again. Several joiners at once on one peer all take their pages.
- AC-7: with the switch on the node answers 503 on ready and its lease carries no `search` until the copied bytes reach `ready_fraction` of the bytes named (fixed when the lists arrive, less a failed peer's share: a fake peer that names N bytes and fails after M leaves the target at 0.9 of the other peers' bytes plus M) or `max_wait`; then 200 and `search`. With it off, 200 at once. A `max_wait` above 5 minutes is clamped.
- AC-9: the joiner's validation: a page whose key is outside the shard's namespace, on another term or version, decoded, of a pinned name, with `page = u32::MAX`, or of the wrong length leaves the joiner's cache footprint, `metadata_bytes` and memory unchanged and is not counted toward ready.
- AC-8: provisional, on the emulator with latency: joiner GETs over the whole warm-up and time to first-query p50, and peers' search p99 (also while a 1,000-shard listing runs), bytes served and CPU, with the copy on against off, five runs each with their spread, in VERIFIED.md; the switch's default follows the build rule.
- AC-10: a peer bounds the bytes it serves to joiners, across all of them, at `serve_bytes_per_second` (`OSS_WARM_SERVE_BYTES_PER_SECOND`): a page batch is answered after a wait so that the bytes served, over any window, add up to at most the rate (plus one batch), through an injected clock; the wait is taken while the request holds its `max_concurrent` permit, so the queue behind it is bounded by that limit; a request the permit limit refuses is refused as before; the hot-set listing is not charged. The rate never lowers what a single joiner at its own `max_bytes_per_second` is served when that is at or under the peer's rate (a test: one joiner at the rate is never made to wait).
- AC-11: the defaults of `serve_bytes_per_second`, the joiner's `max_bytes_per_second`, `parallel_peers`, `max_concurrent` and `open_parallel` come from a sweep recorded in VERIFIED.md: for a reference node (r8g.4xlarge: 16 vCPU, 128 GiB, network up to 15 Gbps, EBS up to 10 Gbps, per AWS's published table) and a stated hot-set size, the peer's p99 during the join against the copy-off arm, the joiner's time to ready, and the share of the named bytes held at `max_wait`, for each setting value. If holding the peer's p99 and finishing inside `max_wait` conflict for a realistic hot set, VERIFIED.md says so and which one the default gives up.

## Test plan

AC-1: `crates/node/tests` over the emulator. AC-2, AC-3: `oss-node` tests with a stub membership, a counting store. AC-4: injected clock and `Pause`. AC-5, AC-7: two-node boot tests in `crates/node/tests/server/`. AC-6: fake peers over the transport. AC-8: an ignored evidence test. AC-10: `warm_handlers` tests with an injected clock and a fixed-rate stub. AC-11: the ignored sweep in `warm_interleaved_evidence.rs`.

## S3 request budget

The copy adds no request: a peer serves only what it holds, a joiner stores what it is handed. It removes the GETs a joiner makes for those pages when searched. Over the whole warm-up the joiner makes the GETs of the pages no peer handed over only when searched, as before. LIST 0, HEAD 0, PUT 0; a node that is never searched makes none. A row in the request budget table for the warm-up window.

## Memory

The disk tier lives on disk; its page records are in memory (`metadata_bytes`), charged to `open_indexes`. The joiner reserves `batch bytes × parallel_peers` (default 4 MiB × 4) before asking; a peer holds one batch while answering (at most `max_concurrent` × 4 MiB), reserved from the same pool; a peer under pressure refuses.

## Switch

`serverless.search.warm_on_join.enabled` (cluster, per index), `ready_fraction`, `max_wait`, `max_bytes_per_second`, `max_shards`, `max_bytes`, `max_pages_per_request`, `max_concurrent`, `parallel_peers`, `serve_bytes_per_second`; and the disk tier's `OSS_PAGE_CACHE_DIR` and `OSS_PAGE_CACHE_BYTES`. All default off; entries in `docs/how-to`.
