# Transport secret recordings

The transport secret register's value as the shell's own `TransportSecrets` (huyz0/OpenSearch `serverless/shell/src/main/java/org/opensearch/serverless/metadata/TransportSecrets.java` at `429b4cbc`) reads and writes it. `crates/format/tests/transport_secrets.rs` reads the same inputs in Rust and checks it gets the same fields and bytes.

`TransportSecretRecordings.java` compiles against that one shell file and the jars `fixtures/cluster-config/README.md` lists:

```bash
CP="$(ls jars/*.jar | tr '\n' ':')"
javac -proc:none -cp "$CP" -sourcepath src -d out fixtures/transport-secret/TransportSecretRecordings.java
java -cp "$CP:out" org.opensearch.serverless.metadata.TransportSecretRecordings fixtures/transport-secret
```

| File | What |
|---|---|
| `cases.txt` | values to read, one a line, written by hand: bare secrets, objects with every field and with defaults, numbers of each kind, unknown fields, escapes, and values the Java refuses |
| `parsed.txt` | for each case, `generation current previous rotated_at` as the Java read it (keys as UTF-8 hex, `~` for none), or `refused` |
| `written.txt` | three values, `generation current previous rotated_at`, then the JSON the Java's `toJson` wrote, checked to read back the same |
