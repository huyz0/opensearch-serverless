# Routing recordings

What the shell's routing classes answer: `RoutingHash`, `UnitLayout`, `UnitLayoutFormat`, `ShardMode` and `RoutingSettings` from huyz0/OpenSearch `serverless/shell/src/main/java/org/opensearch/serverless/routing/` at `429b4cbc`, with `settings/SettingsProvider`, compiled unchanged against the OpenSearch 3.3.0 jars that `fixtures/cluster-config/README.md` lists, and core's `OperationRouting.generateShardId` over the `IndexMetadata` the shell renders for an index without a shard mode (routing shards equal to the shard count). `crates/cluster/tests/routing/` checks every answer.

```bash
for f in routing/ShardMode routing/RoutingHash routing/UnitLayout routing/UnitLayoutFormat routing/RoutingSettings settings/SettingsProvider; do
  mkdir -p src/org/opensearch/serverless/$(dirname $f)
  git -C <OpenSearch checkout> show 429b4cbc:serverless/shell/src/main/java/org/opensearch/serverless/$f.java \
    > src/org/opensearch/serverless/$f.java
done
CP="$(ls jars/*.jar | tr '\n' ':')"
javac -proc:none -cp "$CP" -d out $(find src -name '*.java') fixtures/routing/Routings.java
java -cp "$CP:out" org.opensearch.serverless.routing.Routings fixtures/routing
```

| File | What |
|---|---|
| `hashes.txt` | 10,000 ids from a seeded random (empty, ASCII, numbers, UUIDs, accented and CJK text, surrogate pairs, up to 300 characters) as UTF-8 hex (`-` for empty): the 64-bit routing hash, core's 32-bit murmur3, and the declared shard at 1, 2, 3, 5, 7, 30, 1000 and 4096 shards |
| `bounds.txt` | declared shard count, shard, and its first and last hash |
| `legacy.txt` | the same ids through `generateShardId` for twelve (shards, routing partition) pairs, a third with a routing value and every partitioned one with one (`~` for none) |
| `layouts.txt` | 36 layouts (fixed and adaptive as created, and grown by seeded random splits, merges and pending splits) as `toJson` writes them, each followed by `unitFor` at every unit's ends and beyond, `planSplit` of every unit, `unitsOf` with `needsFilter` for every declared shard, the merge of every neighbouring pair, and the pending split cleared or applied |
| `units.txt` | three layouts (fixed of 5, adaptive grown, fixed of 3 with a declared shard halved) and the unit each id routes to in each |
| `refusals.txt` | 47 layout texts and what `fromJson` then `toJson` makes of each, `refused` when it throws |
