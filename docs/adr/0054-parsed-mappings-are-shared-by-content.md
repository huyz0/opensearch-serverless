# 0054: Parsed mappings are shared across shards by content, in memory; the descriptor keeps its mapping inline

**Status:** Accepted after independent review (one round, changes applied), 2026-11-24

## Context

The goal is about 100 distinct schemas across about 10M tenants of about 10 indexes each. Every open shard on a node holds its index's mapping parsed: the writer's `oss_doc::Mapping` in the shard, and the reader's `Mapping` and mapping source text in `IndexMeta`. Measured once with a throwaway counting allocator (provisional: a Linux debug build in a container, deterministic byte counts, not a timing; not kept as a test, because a counting allocator is `unsafe` and outside the patterns of [docs/unsafe.md](../unsafe.md)): a 150-field mapping whose JSON is 4,220 bytes keeps **209,946 bytes** parsed, about 1.4 KB a field. A node holding 20,000 open shards of 100 schemas pays 4.2 GB for copies that are byte-identical in 100 groups, and about 21 MB if they were shared. The Java shell measured the same effect (182 to 92 KB an open index at 3K indexes, 2026-11-12) by sharing analyzers and mappings behind `serverless.schema.sharing.enabled`, and parked a second step: a schema blob `schemas/<hash>` that the descriptor names by hash.

Forces: requests scale with nodes and bytes, never with indexes; memory is the per-node ceiling that decides how many shards a node holds open; a Java node reads the same descriptors (a changed descriptor format is a mixed-fleet break); a feature with no measured downside ships without a switch.

## Decision

**Share the parsed mapping in memory, keyed by the mapping's exact text (the descriptor's parse options are fixed; a varying one joins the key), between shards opened from a descriptor. Leave the descriptor as it is.**

1. A node-wide `MappingInterner` maps `(parse options, mapping text)` to a weak reference to the parsed `Arc<Mapping>`. Opening a shard looks its descriptor's text up; a hit returns the same `Arc`, a miss parses once and inserts. A hit costs one hash of the text and one equality check, and no request. The options are part of the key because `Mapping::from_descriptor` is a pure function of the text and its fixed options (`knn` on), and nothing about the index (name, uuid, settings) reaches the parse; if a later parse option comes from an index setting, it belongs in the key.
2. The key is the full text, compared for equality on a hit, so a collision cannot hand a shard another mapping. Two mappings that mean the same but are spelled differently are two entries: sharing is by identical text, which templates and an application's repeated creates produce. No canonical form.
3. **Memory of the table.** The key is an `Arc<str>` the table owns, one per distinct live text, about the length of the descriptor text the node already read to open the shard (4 to 15 KB); the table holds one `Weak` and one key per distinct live mapping and nothing per shard. It is not charged to the request pool: it is bounded by live distinct mappings, each also held by an open shard whose own open already reserves. Each insert carries an `// ALLOC:` bound. Dead entries (the shards closed) are removed by the lookup that finds the `Weak` dead, and by a sweep of one lock shard when its length passes twice the live count the last sweep left (and at least 64); the threshold resets after each sweep, so mass closes do not strand keys. A sweep walks one lock shard under its own lock, never the whole table.
4. The table is split in 16 lock shards by the hash, each behind a mutex that recovers a poisoned lock (the repo's `check-poison` rule); a miss parses outside the lock and inserts after, so a slow parse never blocks a hit, and two racing misses keep one result.
5. **A shard's mapping is an `Arc<Mapping>`, immutable.** A dynamic mapping update builds the grown mapping by value (as `Mapping::merge` does now) and the shard takes it as its own `Arc`: the shard no longer shares, it keeps the grown copy and pays the parse's footprint, as the Java measured of a drifted index. The first slice does not re-intern a grown mapping (that would cost a `to_json`, a hash and a possible re-parse of a mapping already in memory per growing write, for indexes that happen to drift to the same text); the existing clone of the grown mapping on a growing write stays (the parsed document still needs it for that write) and is now this shard's own `Arc`. Anything that wants to change a shared mapping in place fails to compile.
6. **No switch.** Sharing changes no request, format or answer; its cost is a hash a shard open and one uncontended-in-practice lock. It becomes a setting if a measured shard-open or write-p99 regression over 5% appears (the repo's own bar), and the spec holds a relative bound that a hit costs less than a tenth of a parse.

### Not decided here: shrinking the mapping itself

The review found where the 1.4 KB a field comes from: a leaf field stores one `Value` for every parameter its type declares, defaults included (up to 17), in `Leaf::new`. Storing only the parameters a mapping set, and computing the defaults on read, would shrink every parsed mapping, including a drifted index's own copy, which sharing cannot help. That is a separate change with its own spec and a before and after measurement (it needs a harness without `unsafe`, or a new pattern in docs/unsafe.md decided then); it composes with this one and is listed as the next task.

### Alternatives rejected

- **A schema blob `schemas/<hash>` named by the descriptor** (the Java's parked step). It saves descriptor bytes only: 4 to 15 KB times 100M indexes is 0.4 to 1.5 TB at about $0.023 a GB-month, $10 to $35 a month (arithmetic, not a measurement). It costs a conditional PUT on a create (or a HEAD to know it exists), a GET on a cold open the descriptor GET did not need, a janitor reference count for blobs no descriptor names, and a new descriptor field. A Rust node skips unknown descriptor fields (`parse_index` is forward compatible); whether a Java node that rewrites a descriptor keeps one is not verified here, and the saving does not justify finding out. Revisit only if descriptor bytes are measured to matter.
- **Field-level sharing or base plus delta.** Opt-in at most, per the owner's earlier decision, and only if measurements show whole-mapping sharing collapsing at moderate drift. Not built.
- **A canonical form (sorted keys, stripped whitespace) as the key.** A few more groups at the price of a parser that must agree with the Java's byte for byte.
- **Re-interning a grown mapping by text.** See point 5.

## Consequences

- Requests: none added. Memory per shard: one `Arc` in place of the parsed copy; the node holds each distinct live mapping once. The saving is proportional to shards per distinct mapping and is zero when every index has its own or drifts.
- A mapping parse that fails is not stored; the open fails as it does now, and a retry parses again as it does now.
- The table is process-local; a restart parses each distinct mapping once.
- Slices: the writer's shard first (the `Shard::new` signature takes `impl Into<Arc<Mapping>>`, so existing callers compile; the opener, slot and takeover carry an `Arc`), then the reader's `IndexMeta`, whose `mapping: Mapping` becomes an `Arc`.
- Not done: the descriptor is not smaller; the reader's per-segment state is separate.
