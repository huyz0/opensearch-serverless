# Porting

Most of what this repository does already exists in Java: OpenSearch itself, and the serverless shell in huyz0/OpenSearch (branch `feature/serverlessshell`). Port that code clearly instead of writing it from scratch, and keep every Rust module traceable to the Java it came from.

## Sources

| Source | What comes from it | Pinned at |
|---|---|---|
| huyz0/OpenSearch `feature/serverlessshell`, `serverless/` | blob formats, node write log, fencing, descriptors, publish, push, takeover | the commit named in the port map row |
| OpenSearch (the version the shell builds on) | REST handlers, query DSL parsing, mappings, reduce, response shapes | the same |
| lucene-rust | the engine; used as a dependency, not ported | `Cargo.toml` revision |

## How to port a piece

1. **Find the Java source** and record it in [port-map.tsv](port-map.tsv) before writing code: the Rust path, `port`, the Java path at its pinned commit, and a note.
2. **Port by behaviour and format, not class hierarchy.** The blob formats and the REST contract are the compatibility surface; the in-memory design is Rust's. Keep the Java's algorithm and constants unless there is a measured reason not to, and say so in the module doc.
3. **Cite the Java in the module doc**: `Ported from serverless/shell/src/main/java/org/opensearch/serverless/store/SegmentBlobs.java`. Names stay recognisable so a reader can open both side by side.
4. **Test against the Java's behaviour**: blob fixtures written by the Java shell, and REST compatibility fixtures recorded from OpenSearch ([compatibility.md](compatibility.md)).
5. **Measure** where the Java was measured; a port slower than the Java on the same workload is a bug.

Code with no Java counterpart is marked `new` in the port map with a reason.

## Enforced by

`scripts/check-port-map.py`: every Rust source file under `crates/*/src` has a row; every row names a file that exists; a `port` row names its Java source; a `new` row says why. It cannot check that the Java path exists at the pinned commit (the Java tree is not in this repository).
