# Jackson recordings

JSON written by Jackson 3 (`tools.jackson.core:jackson-core:3.2.3`, the version huyz0/OpenSearch pins at `3920dc36` in `gradle/libs.versions.toml`), configured as OpenSearch's `JsonXContent`. `crates/format/tests/records.rs` checks that the Rust writers produce the same bytes.

| File | What |
|---|---|
| `every-character.json` | one string holding U+0000 to U+007F, then U+0080, U+009F, U+00E9, U+2028, U+2029, U+1F600 and U+FEFF, as `JsonGenerator.writeString` escapes it |
| `descriptor-layout.json` | an index descriptor with every field `IndexDescriptor.toBytes` writes but `bucket`, in its order: a mapping holding escapes and non-ASCII, both versions moved, creation time and version, two aliases, a unit layout and nested settings with a list |
| `descriptor-bucket.json` | a bucketed one-shard descriptor with no mapping, extreme numbers and empty settings |

`Record.java` writes them field by field in `toBytes`'s order (the generator calls `XContentBuilder` makes), since running `IndexDescriptor` itself needs the whole shell build:

```bash
curl -sSfO https://repo1.maven.org/maven2/tools/jackson/core/jackson-core/3.2.3/jackson-core-3.2.3.jar
java -cp jackson-core-3.2.3.jar Record.java fixtures/jackson
```

The descriptor the shell itself wrote is `fixtures/shell-blobs/indices/fixture`.
