# Compatibility blobs

The indices of the search fixtures in [fixtures/compat/search/](../compat/search/), written through the Java serverless shell, so a reader node serves the fixtures' `_search` steps from what the shell published (`crates/compat/tests/node_search.rs`, `crates/node/tests/`). Each file sits at its blob key; registers are the filesystem store's (an 8-byte generation, then the value).

| Key | What |
|---|---|
| `indices/<index>` | the index descriptor register: uuid `<index>-uuid`, one shard, the fixture's mapping |
| `segments/<index>#<index>-uuid#0/manifest` | the shard's commit register, term 1 |
| `segments/<index>#<index>-uuid#0/t=1/_<n>.seg` | one blob a segment; `compat-paging-sort`'s documents landed in two segments, so its searches span both, and every other index has one |

Only the descriptor, commit and segment blobs are kept; node logs, leases and the other blobs the run left are not read.

## How they were made

From huyz0/OpenSearch branch `claude/project-thread-g7abaw` (commit `d34eba27`, which adds one test on top of `feature/serverlessshell` at `3920dc36`). `CompatBlobFixtureTests` reads each fixture's `PUT /<index>` mapping and `_bulk` documents, indexes them in order with their explicit ids on a node with node packs off, publishes every index once, and copies the blobs:

```bash
# Build setup: the project's build-setup/README.md (Maven mirror init script, non-root `builder` user).
./gradlew -Dbuild.docker=false -Djapicmp.compare.version=3.3.0 \
  :serverless:testkit:test --tests org.opensearch.serverless.testkit.CompatBlobFixtureTests \
  -Dtests.compat_blobs.fixtures=<this repo>/fixtures/compat/search \
  -Dtests.compat_blobs.out=/home/builder/cb -Dtests.security.manager=false -Dtests.seed=40FE84DA0BFC04FA
```

Rerun it after a search fixture's mapping or documents change; the recorded responses (`fixtures/compat/record.py`) and these blobs must come from the same requests.
