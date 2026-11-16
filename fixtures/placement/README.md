# Placement recordings

What the shell's placement classes answer for one fixed fleet: `Rendezvous`, `ReaderPlacement`, `HotShardSpread`, `ZonePreference` and `Zones` from huyz0/OpenSearch `serverless/shell/src/main/java/org/opensearch/serverless/cluster/` at `1455ff58`, with `membership/NodeLease`, `nodelog/NodeLogEpoch` and `settings/SettingsProvider`, compiled unchanged against the OpenSearch 3.3.0 jars that `fixtures/cluster-config/README.md` lists. `node/FleetPlacement` needs the metadata plane to compile, so its decisions are copied into `Placements.java` over a fixed view, line for line. `crates/cluster/tests/placement/` reads the same fleet in Rust and checks every answer.

```bash
for f in cluster/Rendezvous cluster/ReaderPlacement cluster/ReaderPlacementSettings cluster/HotShardSpread \
         cluster/ZonePreference cluster/Zones membership/NodeLease nodelog/NodeLogEpoch settings/SettingsProvider; do
  mkdir -p src/org/opensearch/serverless/$(dirname $f)
  git -C <OpenSearch checkout> show 1455ff58:serverless/shell/src/main/java/org/opensearch/serverless/$f.java \
    > src/org/opensearch/serverless/$f.java
done
CP="$(ls jars/*.jar | tr '\n' ':')"
javac -proc:none -cp "$CP" -d out $(find src -name '*.java') fixtures/placement/Placements.java
java -cp "$CP:out" org.opensearch.serverless.cluster.Placements fixtures/placement
```

| File | What |
|---|---|
| `fleet.txt` | the twelve leases, as `toBytes` writes them: mixed roles and zones, one revoked, one expired within a lifetime and one beyond, one with no load, ids whose order is not by length (`N3` sorts before `n01`) |
| `hashes.txt` | a string in hex (`-` for empty), `Murmur3HashFunction.hash` of it, and its `hashCode` |
| `owners.txt` | a writer key, the candidates, and `Rendezvous.owner` |
| `readers.txt` | index, shard, role, limit, and `ReaderPlacement.candidatesFor` over the fleet |
| `zones.txt` | index, shard, local zone (`-` for none), searches counted, searches running on placement's first reader, placement's two readers, and `ZonePreference.order` |
| `fleet-decisions.txt` | for three local nodes at three loads: fleet size, member with room, mean held, member below mean, hand-off target, the writer for each shard, and the takeover winner for dead owners `n02` and `n07` (in the view), `gone-a` (departed, zone known) and `gone-x` (departed, unknown), at overcommit 0 and 0.5 |
| `boundaries.txt` | 400 small fleets from a seeded random (members as `id:roles:zone:held:cap:in_use:refused:expires-from-now:revoked`, `;`-separated), with loads drawn around each threshold, and for each: local id, held, cap, overcommit, fleet size, member with room, mean held, member below mean, above fleet share, hand-off target, writer for shard `logs/<row>`, a dead owner and its takeover winner |
