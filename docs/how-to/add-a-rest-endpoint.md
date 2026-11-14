# Add a REST endpoint

REST lives in `oss-coordinator`, built in the coordinator stage ([roadmap](../roadmap.md)). Until then the Java coordinator serves REST and routes supported searches to the Rust reader.

When it exists:

1. One module per endpoint family under `crates/coordinator/src/rest/`.
2. Declare the endpoint's compatibility: which OpenSearch parameters it matches, which it changes, which it refuses. Unknown parameters fail explicitly; they are never ignored.
3. Request handling parses, routes and reduces; shard work belongs to the reader or writer.
4. Test it against the in-process emulator, including the request count the endpoint costs.

**Checks:** layering keeps shard logic out of the coordinator; tests prove the rest.
