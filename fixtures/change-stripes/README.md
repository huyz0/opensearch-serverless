# Change-stripe recordings

Change stripes written by the Java's own code: `Stripes.java` copies `State.add`, `State.parse`, `State.encode` and `stripeOf` verbatim from huyz0/OpenSearch `serverless/shell/src/main/java/org/opensearch/serverless/metadata/ChangeStripes.java` at `3920dc36`, since running `ChangeStripes` itself needs the whole shell build. `crates/format/tests/change_stripes.rs` replays the same bumps in Rust and checks the bytes match.

```bash
java fixtures/change-stripes/Stripes.java fixtures/change-stripes
```

| File | What |
|---|---|
| `bumps.txt` | a delete's two bumps with an update between them, a finished delete, and a name with spaces and non-ASCII |
| `trimmed.txt` | 300 applied changes with an unfinished delete among the oldest: the newest 256 applied kept, and the delete |
| `new-epoch.txt` | 65 unfinished deletes move the epoch on, then one change |
| `reencoded.txt` | a stripe with a trailing space in its head, blank lines and an unknown kind, parsed and encoded again |
| `stripe-of.txt` | `stripeOf` of names including the empty one, non-ASCII and a character outside the BMP |
