# Virtual-time soak: a week of lifecycle in minutes

## Serves

Parity plan R15.5 (a week-long soak). A real week is 604,800 s; what a week finds is
what happens when time passes: leases expire and are taken over, node logs rotate and
are collected, orphans age past their sightings, point-in-time contexts expire, and
the request count per idle index stays flat. Those are functions of the clock, so
they run on a simulated one. What a clock cannot show is run for real, briefly.

## What exists today

- `oss_cluster::LeaseClock` (`now_millis`, `pause`) is injected into membership,
  the node's boot, health, tail and forward paths, and is faked in the cluster and
  janitor tests.
- The janitor, the descriptor store's alias clock and the PIT store take the time as
  an argument or a closure (`PitClock`); the janitor's `node_log` tests already drive
  a collector on a stepped clock over the emulator.
- Not injected: 28 files read `SystemTime::now` directly (the writer's stamps in
  `writer/mod.rs`, `search_open.rs`, document and bulk routes, by-query) and 174 read
  `Instant::now` (deadlines, rate limits, TTL caches, timeouts). `Emulator` latency is
  real sleep, not virtual.
- The pack collector (R11.2) is not built, so it is not in this soak until it is.

## Change

A test crate-level rig, `crates/node/tests/virtual_soak/` (own binary, `#[ignore]`d
only for its long profile), and nothing in product code except the smallest seams
needed to reach a clock a lifecycle step reads:

1. A `VirtualClock` implementing `LeaseClock`: `now_millis` is a shared atomic;
   `pause` advances it by the pause instead of sleeping. One clock is handed to every
   node of an in-process fleet, the janitor and the PIT store.
2. A driver that advances the clock in steps (default 60 s, 10,080 steps = 7 days)
   and, at each step, runs what a live node's timers would: renew leases, one
   janitor pass, PIT expiry, node-log rotation. A seeded schedule kills, freezes and
   restarts nodes and writes batches at a seeded rate; a ledger records every
   acknowledged document.
3. Seams: where a lifecycle step reads `SystemTime::now` or `Instant::now`
   directly, it takes a clock argument with the system clock as the default (listed
   in VERIFIED.md as each is found; no behaviour change, every existing test pins
   the old one).

The store is the in-process emulator with latency off (`latency=none`), counting
every request.

## Acceptance criteria

- AC-1: over 7 virtual days of at least 20 seeded node kills, freezes and restarts,
  every acknowledged document is present exactly once on every live node after each
  take-over (the ledger judges it).
- AC-2: a dead node's lease is taken over within the lease TTL plus the pass interval
  of virtual time, for every kill in the run; the slowest of the last third is no more
  than twice the slowest of the first.
- AC-3: node logs rotate and the janitor collects every dead node's log: after the run
  no log of a node dead for more than two TTLs remains, and none of a live node's
  is deleted.
- AC-4: orphans younger than the sighting window are never deleted, and every one
  older is, after the number of sightings the janitor requires.
- AC-5: PIT contexts expire at their keep-alive in virtual time and no earlier; an
  expired one answers the typed error.
- AC-6: an idle index makes zero store requests over the whole 7 virtual days (the
  meter's per-index count), and the run's total requests per virtual hour do not grow
  between its first and last day (provisional, emulator).
- AC-7: a short real-time smoke run (60 s wall, 2 s TTL, real `SystemClock`, same
  driver) passes AC-1 to AC-3 with the compressed numbers: it is run in CI and holds
  what the virtual run cannot (below).

## What it proves, and what it does not

It proves: the lifecycle logic is correct for any ordering of kills, freezes and
timer firings the seed produces, over weeks of virtual time; no per-index request
grows with elapsed time; leases, sightings and keep-alives use the injected clock
everywhere a lifecycle step reads it.

It does not prove, and R15.5 stays **built, not done** until these are covered or
declared out of scope by the owner:

- Resource drift that needs real time: memory growth, file-descriptor and thread
  leaks, allocator fragmentation, page-cache churn, and disk fill. A virtual week
  runs in minutes of CPU, so these show as minutes, not as a week. The short real-time
  run and the existing heap tests bound them only for their duration.
- Real latency, timeouts and rate limits (`Instant::now` paths: forward timeouts,
  queue waits, backoff), which a virtual clock does not advance until they are given
  the seam.
- A real store: RustFS and S3 behaviour over days (listing consistency, conditional
  create under load, throttling). RustFS runs stay the cluster and takeover suites.
- Mixed fleet: Java nodes run real time; the 98-cycle live run stays the evidence.
- Clock skew and backward steps between nodes, unless the driver gives each node its
  own offset clock (AC-2 runs a second pass with offsets of up to one TTL).
- The pack collector, until R11.2 exists.

## Test plan

Each AC is first seen red by planting the bug it catches: a lease renewal that
ignores the clock (AC-2), a janitor that deletes at one sighting (AC-4), a PIT that
never expires (AC-5), a per-index timer that requests every pass (AC-6).

## S3 request budget

Unchanged: the test observes. AC-6 asserts zero for an idle index.

## Switch

None: tests, and clock arguments whose default is the system clock.

## Risks

- A seam that is missed makes a lifecycle step run on real time inside a virtual run:
  the run then stalls or passes for the wrong reason. The driver fails if any
  virtual step takes more than 5 s of wall time, which shows a stalled real-time wait.
- The driver can mask a race by serialising what timers run concurrently; the
  real-time smoke run (AC-7) is the check for that.
