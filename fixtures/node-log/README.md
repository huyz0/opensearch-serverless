# Node-log recordings

Write-log records and node-log blobs written by the shell's own classes: `WalRecord` and `NodeLogBlob` from huyz0/OpenSearch `serverless/shell/src/main/java/org/opensearch/serverless/nodelog/` at `429b4cbc`, compiled unchanged against that revision's own `libs/core`, `libs/common` and `libs/x-content` sources (Jackson 3, as the shell runs; OpenSearch 3.3.0's Jackson 2 escapes a character past U+FFFF as two `\u` surrogates, which the shell does not). `SequenceNumbers` and `Versions` are server classes; `stub/` holds just the three constants these two classes read, copied from that revision. `crates/format/tests/node_log.rs` builds the same inputs in Rust and checks it gets the same records, blobs and headers.

It needs from Maven Central: `tools.jackson.core:jackson-core` and `tools.jackson.dataformat:jackson-dataformat-{smile,yaml,cbor}` 3.2.3, `org.snakeyaml:snakeyaml-engine` 3.1.1, `log4j-api` 2.25.5, `lucene-core` 10.5.1, `jackson-annotations` 2.22 and `jsr305` 3.0.2.

```bash
O=<OpenSearch checkout at 429b4cbc>
mkdir -p src/org/opensearch/serverless/nodelog
for f in NodeLogBlob WalRecord; do
  git -C $O show 429b4cbc:serverless/shell/src/main/java/org/opensearch/serverless/nodelog/$f.java \
    > src/org/opensearch/serverless/nodelog/$f.java
done

CP="$(ls jars/*.jar | tr '\n' ':')"
javac -proc:none -cp "$CP" -sourcepath "fixtures/node-log/stub:$O/libs/core/src/main/java:$O/libs/common/src/main/java:$O/libs/x-content/src/main/java" \
  -d out src/org/opensearch/serverless/nodelog/*.java fixtures/node-log/NodeLogs.java \
  $O/libs/x-content/src/main/java/org/opensearch/common/xcontent/spi/XContentProvider.java
mkdir -p out/META-INF/services
cp $O/libs/x-content/src/main/resources/META-INF/services/org.opensearch.core.xcontent.spi.MediaTypeProvider out/META-INF/services/
java -cp "$CP:out" org.opensearch.serverless.nodelog.NodeLogs fixtures/node-log
```

| File | What |
|---|---|
| `records.txt` | records to read, one a line, written by hand: every field, unknown fields of every type, partial sequence identities, extreme numbers, escapes and characters past U+FFFF, whitespace, and records the Java refuses (a missing field, an empty routing, a repeated field, not an object) |
| `records-parsed.txt` | for each, what `toBytes` writes for the record read, checked to read back to the same bytes, or `refused` |
| `records-strict.txt` | records the Java reads by coercing a type it never writes (a number as text, text as a number or flag, a fraction, `null`), or refuses, and one followed by other bytes; the Rust refuses each |
| `records-strict-parsed.txt` | what the Java makes of each |
| `blob-cases.txt` | windows to encode, in a small line format its header explains: prior running indexes, batches by record line, trims, empty batches, names known late, uuids whose UTF-16 and UTF-8 orders differ, cases the Java refuses, extreme numbers, and fences |
| `blobs.txt` | for each case, `NodeLogBlob.encode` or `encodeFence` in hex, or `refused` |
| `decoded.txt` | for each, what `NodeLogBlob.decode` reads back from those bytes; the recorder also checks each slice parses back to its batch's records |
