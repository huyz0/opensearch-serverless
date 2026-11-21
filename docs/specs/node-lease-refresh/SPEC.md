# How often a node reads the fleet's leases is a setting

## Serves

Parity plan R9.4 and the lease view's correctness: a node reads the other nodes' leases on the thread that renews its own, once every third of a lease's lifetime (10 s at the default 30 s). That period fixes how long a join goes unseen, how late a node notices a peer's expiry, and the background GETs of an idle node. It was not settable, and it was tied to the renewal (a PUT), so reading more often meant renewing more often.

## Change

- `OSS_LEASE_REFRESH_MS` sets how often the node reads the fleet's leases again. Unset, it is the renewal period (a third of `OSS_LEASE_TTL_MS`), so a node behaves as before. A value that is not a whole number of milliseconds, or is zero, is refused by name.
- The lease thread ticks as often as the shorter of the two periods; the lease is renewed on its own period and the fleet read on its own (`boot::renew::Cadence`). A read that fails is tried again at the next tick; a renewal that fails waits for the next period, as before.
- A meter, `crates/node/tests/lease_poll_meter.rs`, counts an idle node's requests and times how long a join goes unseen under a chosen interval (`LEASE_POLL_MS`); the freshness meter takes `METER_REFRESH_MS`.

## Acceptance criteria

- AC-1: with the variable unset the refresh period is the renewal period; set, it is that many milliseconds and the renewal period is unchanged.
- AC-2: a zero or unparsable value is refused naming `OSS_LEASE_REFRESH_MS`.
- AC-3: with equal periods every tick renews and reads; a shorter read period ticks at it and renews on its own period; a longer one ticks at the renewal and reads at the first tick past its period.
- AC-4: a read that failed is due again at the next tick; a renewal is due once a period whether or not it succeeded.
- AC-5: the default does not change: the meter at the default and the node's idle requests are what they were before this change.
- AC-6: the announce message (`internal:serverless/lease/announce`, the joiner's node id) round-trips and a truncated or empty one is refused.
- AC-7: a node that receives an announce for a node it does not know reads the fleet's leases once and answers that it did; for a node it already holds live, it reads nothing and answers that it did not.
- AC-8: announces are bounded: an unknown announce within `ANNOUNCE_GAP` of a read is answered without one and queues a single read for when the gap ends (which covers every announce in it), so a fleet starting together costs each node at most one read a gap and no join waits for the read period.
- AC-9: a node announces itself once after it joins, to at most `MAX_ANNOUNCE` live peers other than itself, each call under `SEND_TIMEOUT`; a peer that is unreachable, or that does not know the action (a Java node), costs nothing but the call.
- AC-10: the announce is signed as every transport call is; an unsigned one is refused before the handler reads anything.
- AC-11: a booted node reads the fleet once at start-up, so it knows its peers before the first period passes, and a booted peer learns of a joiner within a second without the read period being shortened (the lease poll meter, join pickup, at the default period).

## Joiner announces itself

A join otherwise goes unseen for up to the read period (10 s at the default). A node that has enrolled its lease sends each live peer it knows one small signed message naming itself; a peer that does not hold that node reads the fleet's leases once. The steady state is unchanged (no extra read while nothing joins), a join costs one read a peer, and a fleet that starts together is held to one read a gap a node. The announce is advisory: a peer that misses it picks the join up at its next period, and nothing that decides ownership consults the view.

## Test plan

Unit tests in `crates/node/src/boot/config.rs` and `crates/node/src/boot/renew.rs`; the two slow-tier meters for the measurements in VERIFIED.md.

## S3 request budget

Default unchanged (one fleet read a third of a lifetime). A shorter interval adds GETs and no PUT: measured in VERIFIED.md, a per-node cost independent of the fleet's size (the digest keeps a refresh to about one read), so a cluster pays it times its nodes.

## Switch

`OSS_LEASE_REFRESH_MS`, default unset (a third of the lease lifetime). Measured: a faster read does not shorten the time a write takes to be visible to readers (the tail overlay pushes it), and shortens only how long a join or a peer's expiry goes unseen; at about 2.5 times the GETs for 5x the rate it is not a win for every deployment, so it stays a setting.

## Risks

- A node that reads leases more often reads the index register and digest on every refresh: at 1,000 nodes the cluster-wide GETs are the per-node figure times 1,000 and hit the same few keys; a faster interval should be jittered by start time, which node start-up already does but nothing enforces.
