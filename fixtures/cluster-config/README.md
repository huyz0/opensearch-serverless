# Cluster-config recordings

Cluster settings read and written by OpenSearch 3.3.0's own `Settings`. `ClusterSettings.java` copies `parse`, `toBytes` and the merge in `update` from huyz0/OpenSearch `serverless/shell/src/main/java/org/opensearch/serverless/metadata/ClusterConfig.java` at `3920dc36`, without the register. `crates/format/tests/cluster_settings.rs` reads the same inputs in Rust and checks it gets the same settings.

It needs these jars from Maven Central on the class path: `org.opensearch:opensearch`, `opensearch-common`, `opensearch-core` and `opensearch-x-content` 3.3.0; `jackson-core`, `jackson-dataformat-smile`, `-yaml` and `-cbor` 2.18.2; `snakeyaml` 2.3; `log4j-api` 2.24.3; `lucene-core` 10.3.1.

```bash
java -cp "$(ls jars/*.jar | tr '\n' ':')" fixtures/cluster-config/ClusterSettings.java fixtures/cluster-config
```

| File | What |
|---|---|
| `cases.txt` | values to read, one a line, written by hand: nested and flat keys, numbers, booleans, `null`, lists, `p.0` list entries, and values the Java refuses |
| `parsed.txt` | for each case, the Java's settings in the `flat_settings` form, checked to read back the same, or `refused` |
| `updates.txt` | five updates, one a line, applied in turn from no settings: one change a word, `key=value` setting the key and `key` alone removing it |
| `updated.txt` | for each update, two lines: the nested value the Java stores, then the `flat_settings` form of the same settings |
