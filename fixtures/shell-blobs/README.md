# Shell blobs

Blobs the Java serverless shell wrote, decoded by `crates/format/tests/`. Each file sits at its blob key, so a test reads a key the readers derived. Registers here are the filesystem store's: an 8-byte generation, then the value.

| Key | What |
|---|---|
| `indices/fixture` | the index descriptor register: index `fixture`, uuid `fixture-uuid-0000000`, one shard |
| `segments/fixture#fixture-uuid-0000000#0/manifest` | the shard's commit register, term 2: segment `_0` in the node pack, `_1` and `segments_5` in the segment blob |
| `segments/fixture#fixture-uuid-0000000#0/t=2/_1.seg` | the second publish's segment blob |
| `packs/tZ9iGQAAQACwLK7Z_____w/e=00000001794710559389/00000000000000000000` | the first publish's node pack, one block (`_0.seg`) |

The other blobs the run left (node logs, leases, the pack ledger, assignments, digest rollups) are not read yet and are not kept.

## How they were made

From huyz0/OpenSearch branch `claude/project-thread-hti37o` (commit `4c7077d9`, which adds one test on top of `feature/serverlessshell` at `49f238ed`). The test `ShellBlobFixtureTests` writes three documents on a node with node packs on and publishes, releases the shard, then writes two more on a second node with node packs off and publishes, and copies every blob from its filesystem store:

```bash
# Build setup: the project's build-setup/README.md (Maven mirror init script, non-root `builder` user).
./gradlew -Dbuild.docker=false -Djapicmp.compare.version=3.3.0 \
  :serverless:testkit:test --tests org.opensearch.serverless.testkit.ShellBlobFixtureTests \
  -Dtests.shell_blobs.out=/home/builder/fx -Dtests.security.manager=false -Dtests.seed=40FE84DA0BFC04FA
```

The seed fixes the node ids; the pack epoch is the clock at the first node's start, so a new run gives a new epoch and the tests' constants (`crates/format/tests/common/mod.rs`) change with it.
