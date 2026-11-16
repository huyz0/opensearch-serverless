# Alias recordings

Alias records read and written by the shell's own `AliasRecord`, from huyz0/OpenSearch `serverless/shell/src/main/java/org/opensearch/serverless/cluster/AliasRecord.java` at `3920dc36`, compiled unchanged against the OpenSearch 3.3.0 jars. `crates/format/tests/alias_records.rs` reads the same inputs in Rust and checks it gets the same record and bytes.

The jars are those `fixtures/cluster-config/README.md` lists. They carry Jackson 2, which escapes a character outside the BMP as a surrogate pair where the shell's Jackson 3 writes it as UTF-8, so the cases hold none. `fixtures/jackson/every-character.json` pins the escaping against Jackson 3.

```bash
mkdir -p src/org/opensearch/serverless/cluster
git -C <OpenSearch checkout> show 3920dc36:serverless/shell/src/main/java/org/opensearch/serverless/cluster/AliasRecord.java \
  > src/org/opensearch/serverless/cluster/AliasRecord.java
CP="$(ls jars/*.jar | tr '\n' ':')"
javac -proc:none -cp "$CP" -d out src/org/opensearch/serverless/cluster/AliasRecord.java fixtures/aliases/Aliases.java
java -cp "$CP:out" Aliases fixtures/aliases
```

| File | What |
|---|---|
| `cases.txt` | records to read, one a line, written by hand: aliases, write members kept and dropped, data streams, mutations, unknown fields, escapes, and records the Java refuses |
| `parsed.txt` | for each case, two lines: what `toBytes` writes for the record read, then its write index and the index writes go to (`-` for none), whether a creation and a mutation are pending, and whether index `b`'s hint is protected, tab-separated; or `refused` and `-` |
| `strict.txt` | records the Java reads by coercing a JSON type it never writes there, and a record followed by other bytes; the Rust refuses each |
| `strict-parsed.txt` | the same two lines for each, showing the Java reads it |
