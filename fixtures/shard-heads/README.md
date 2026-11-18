# Shard-head recordings

Shard heads read and written by the shell's own `ShardHead` class from huyz0/OpenSearch `serverless/shell/src/main/java/org/opensearch/serverless/metadata/ShardHead.java` at `429b4cbc`, compiled unchanged against the OpenSearch 3.3.0 jars that `fixtures/cluster-config/README.md` lists. `crates/format/tests/shard_heads.rs` reads the same inputs in Rust and checks it gets the same heads and bytes.

```bash
mkdir -p src/org/opensearch/serverless/metadata out
git -C <OpenSearch checkout> show 429b4cbc:serverless/shell/src/main/java/org/opensearch/serverless/metadata/ShardHead.java \
  > src/org/opensearch/serverless/metadata/ShardHead.java
CP="$(ls jars/*.jar | tr '\n' ':')"
javac -proc:none -cp "$CP" -d out src/org/opensearch/serverless/metadata/ShardHead.java
java -cp "$CP:out" fixtures/shard-heads/ShardHeads.java fixtures/shard-heads
```

| File | What |
|---|---|
| `cases.txt` | heads to read, one a line, written by hand: every field, a null owner, writers, unknown fields, escapes, and heads the Java refuses (a missing index, a negative shard, a term below 1, a null index, an array, truncated JSON) |
| `parsed.txt` | for each case, what `toBytes` writes for the head read, or `refused` |

The key `shards/<hex>-<index>#<shard>` is checked against `RegisterMap.shardHeadBlob` over `Murmur3HashFunction` in `crates/cluster/tests/leases/heads.rs`.
