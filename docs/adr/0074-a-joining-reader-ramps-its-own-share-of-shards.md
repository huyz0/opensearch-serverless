# 0074: A joining reader ramps its own share of shards, bounded by a floor and a ceiling in time

**Status:** Proposed

## Context

Readers are ranked per shard by rendezvous hashing over the leases ([placement](../specs/placement/SPEC.md)). A reader that joins an established fleet takes about `1/n` of every shard's first rank the moment its lease says `search`, with an empty cache: the first searches of all those shards read cold ([principles](../principles.md): requests scale with nodes and bytes). Warm copy ([ADR 0049](0049-a-joining-reader-warms-from-its-peers.md)) narrows this but is off by default and needs a disk tier. Every node must compute the same ranking from the leases and its own clock without asking anyone, or placement stops being a cheap hint; a disagreement may only cost a cache miss.

## Decision

A search node's lease carries two optional fields: `search_since` (when it first advertised `search`) and `ramp_permille` (its own judgement of how warm it is, 0 to 1000). Every ranker computes a node's ramp as `max(ceiling, min(published, floor))`, where `floor` and `ceiling` are linear in `now - search_since` over `min_duration` and `max_duration`; with no published value the ramp is the ceiling, and with no `search_since` or the switch off it is 1000. A shard is admitted to a node when `murmur3(node/index~shard) mod 1000` is below its ramp; a node not yet admitted ranks after every admitted one, each group in the existing order. Only integers are used, so the Java shell computes the same numbers (one shared fixture, 216 rows).

The joiner publishes its own ramp: at each lease renewal, a window of at least 20 searches that averaged at most two store GETs a search raises it by 100; fewer searches keep collecting, a colder window holds it. The ceiling guarantees a stuck or idle node is fully ramped by `max_duration`; the floor stops it claiming warmth early. A node that does not count its store usage does not publish a ramp, so time alone bounds it.

The switch is `serverless.search.reader_ramp.max_duration` (`OSS_READER_RAMP_MAX_SECS`), default 0 = off, and `min_duration` (default 30 s). With it off the ranking is unchanged. The default flips only on a measurement of no search regression and no added request, as its own change.

## Alternatives considered

- **A fixed time ramp.** Same agreement, no feedback: too slow for a node warmed by copy, too fast for a cold large fleet.
- **Per-shard readiness published by the joiner.** A shard list does not fit a lease (2 KiB) and any other carrier is a new request path, scaling with shards.
- **Load- or latency-weighted ranking by each coordinator.** Coordinators see different traffic and disagree, which is what a lease-derived rule avoids.
- **Weighted rendezvous with `-ln(u)/w`.** Floating-point logarithms differ in the last bit between Rust and Java, so nodes could disagree; the integer admission value gives the same monotone movement exactly.

## Consequences

- Easier: a joining reader takes its share over minutes, so the shards it starts serving are the ones it has had time to warm; a shard once admitted never moves back as the ramp rises, and only shards moving to the joiner change rank.
- No request added: LIST 0, HEAD 0, PUT 0, GET 0. The two fields add about 60 bytes to a lease already written.
- Harder: nodes with the switch on and off disagree on the order for the ramp's length, costing cache misses; a node whose clock runs far ahead treats a joiner as ramped early (today's behaviour). A cold window holds the ramp rather than lowering it, so a joiner that turns cold later is not demoted. Only forwarded searches are counted by the Rust node and all shard queries by the Java node; each judges only itself.
- Rules out: any per-shard state for the ramp, and any rule that needs two nodes to see the same traffic.
