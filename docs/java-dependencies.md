# Java dependencies of the Rust build

The goal (owner, 2026-11-28): no Java plugin, no Java code, no Java fallback; every plugin native. This page lists what in the Rust build still touches Java, what each plugin family needs to be native, and which Java is a test oracle only. Audited against `main` on 2026-11-28; it is a snapshot, so a row that is out of date is a drift to fix, not a rule.

## Runtime path: Java a deployed Rust node can reach

| Item | Where | Kind | What removing it takes |
|---|---|---|---|
| Plugin proxy | `crates/node/src/plugin_proxy.rs`, [plugin-proxy](specs/plugin-proxy/SPEC.md), [ADR 0046](adr/0046-plugin-requests-are-proxied-to-java-nodes.md) | Forwards seven path prefixes (ISM and alerting and anomaly detection under `/_plugins` and `/_opendistro`, `/_plugins/_ml`) to a Java node. Off unless `OSS_PLUGIN_UPSTREAMS` is set | Delete the module, the `ureq` dependency, `serverless.plugins.upstreams`, `max_request_bytes` and `timeout`, `OSS_PLUGIN_UPSTREAMS`, `Router::prefix` if nothing else uses it, and mark ADR 0046 superseded. Blocked only until ISM, alerting, anomaly detection and ML Commons are native (below); LTR no longer uses it |
| Upstream settings | `crates/node/src/boot/config.rs`, `assemble_config.rs`, `crates/types/src/settings` | `serverless.plugins.upstreams` and its two siblings | Goes with the proxy |
| Java shell as a peer | `crates/transport` (OpenSearch native transport), `crates/node/src/push_link.rs`, `tail_reader_link.rs`, `search_shard_action`, `crates/format` (the shell's blob formats) | A Rust node speaks the Java wire and formats so a mixed fleet works. A fleet of Rust nodes alone needs no Java | A decision, not a deletion: keep as an interop surface for migration, or retire when the last Java node is gone. Nothing here starts or calls Java on its own |
| Java as a document source | `serverless.snapshots.stock.*`, [stock-snapshot-restore](specs/stock-snapshot-restore/SPEC.md) | Reads the files a stock OpenSearch wrote; no Java process | None, it reads bytes |

Nothing else in `crates/` starts a Java process, loads a JVM or calls a Java HTTP API. `Cargo.toml` has no JNI or JVM crate.

## Test and tooling only (allowed)

The owner's ruling (2026-11-28): Java may stay in tests, for compatibility checks and for performance and memory comparison against the Rust build. It may not be in the runtime path, the plugin set or a fallback. Everything below is allowed on that basis; each stays ignored by default and out of `scripts/gate.sh`.

| Item | Where | Needs | Role |
|---|---|---|---|
| Mixed-fleet and takeover tests | `crates/node/tests/mixed_fleet*`, `mixed_takeover.rs`, `mixed_java_reads.rs`, `fixtures/mixed-fleet`, `exit-gate`, `mixed-takeover` | `OSS_JAVA_BIN`, `OSS_JAVA_CP`, `RUSTFS_BIN`; all `#[ignore]` | Prove Rust and Java interoperate on one bucket, and serve as the comparison rig against a Java node. Interop tests are meaningful only while the interop surface above stays; the comparison use does not depend on it |
| Live Java plugin tests | `crates/node/tests/mixed_fleet/live_plugins.rs`, `plugin_proxy*.rs` | Java node and `OSS_PLUGIN_ZIPS` for the live ones; `plugin_proxy*.rs` use a fake upstream | Exercise the proxy; delete with it |
| Recorders | 72 `.java` files under `fixtures/` (for example `search-shard`, `pipeline-aggs`, `snapshots`, `multi-terms`, `mustache`, `terms-error`) | The OpenSearch 3.10 jars, compiled by hand | Produce golden bytes and bodies from stock OpenSearch. Their output is committed; the Rust tests replay it without Java |
| Compat fixtures | `fixtures/compat/**`, `crates/compat` | Nothing at test time | Recorded from stock OpenSearch with the stock plugins; replayed in Rust |
| Differential reference | `port-map.tsv` in each crate, [porting.md](porting.md) | Nothing | Names the Java file each module ports; text only |

Nothing in `scripts/gate.sh`, `cargo test` or `.github/workflows/` needs Java; the Java-needing tests are ignored by default.

## Docs that still say Java is part of the system

`README.md`, `docs/architecture.md`, `docs/roadmap.md`, `docs/porting.md`, `docs/protocols.md`, `docs/deployment.md` and `docs/compatibility.md` describe the build as a strangler beside the Java shell ("Java remains only for features not yet ported, then retires", roadmap stage text). The strangler framing stays true for interop and for the porting method; the "Java remains" and "run in the Java shell" sentences become false as each plugin lands, and go in the same PR that removes the proxy.

## Plugin and API families

"Native" means a Rust node answers it with no Java node anywhere. "Not served" means the Rust node answers 400 `no handler found` and no proxy prefix covers it, so a client has no path at all today.

| Family | State | Evidence | What native still needs |
|---|---|---|---|
| Learning to rank | Native, on by default (`OSS_LTR=false` turns off) | [ltr](specs/ltr/SPEC.md), [ADR 0071](adr/0071-learning-to-rank-is-ported-natively.md) | Done. [ADR 0080](adr/0080-one-plugin-system-three-tiers.md) slice 8 would re-home it as the first plugin-system adopter |
| Index State Management | In progress, native port, five slices | [ism](specs/ism/SPEC.md), [ADR 0077](adr/0077-index-state-management-is-ported-natively.md); `crates/node/src/ism_route`, `ism_setting.rs` | Remaining slices; then drop `/_plugins/_ism` and `/_opendistro/_ism` from the proxy. Thread: ISM |
| Alerting | In progress, native port, six slices; slice 1 (registry shards, run-slot claims) merged | [alerting](specs/alerting/SPEC.md), [ADR 0075](adr/0075-alerting-is-ported-natively.md) | Remaining slices (routes, scheduler, executor); Painless trigger scripts need the Painless front end ([ADR 0069](adr/0069-painless-source-compiles-in-rust-with-java-classes-as-functions.md)); the Notifications channels it uses are a separate row below. Thread: alerting |
| Anomaly detection | In progress, native port, six slices | [anomaly-detection](specs/anomaly-detection/SPEC.md), [ADR 0078](adr/0078-anomaly-detection-is-ported-natively.md), `crates/anomaly` | Slice 4 (scheduled runs) blocked on alerting's scheduler. A double field is refused until the reader has one. Thread: anomaly detection |
| ML Commons | In progress, native port, five slices | [ml-commons](specs/ml-commons/SPEC.md), [ADR 0079](adr/0079-ml-commons-is-ported-natively.md) | Model groups, connectors, remote and local models, agents, memory. Reuses `lucene-ml` for ONNX and tokenizers. Thread: ML Commons (the owner asked for a full revamp with no Java) |
| Security (config, authn, authz) | Native for the subset, not proxied | [security-complete](specs/security-complete/SPEC.md), [security-config](specs/security-config/SPEC.md), [security-authn](specs/security-authn/SPEC.md), [security-authz](specs/security-authz/SPEC.md), [ADR 0043](adr/0043-security-configuration-as-document-sets.md), [ADR 0051](adr/0051-authorization-at-the-route-by-action-table.md) | Basic auth only. List form and `PATCH` of the five APIs are native ([security-complete](specs/security-complete/SPEC.md), slices 1 to 10 planned). Not served: hidden and reserved documents, `securityconfig`, `authinfo`, `whoami`, account API, audit logging, document and field level security, tenancy in Dashboards, JWT, OIDC, SAML, LDAP, client certificates, TLS configuration APIs |
| k-NN | Native for the Lucene engine, not proxied | [knn-vector-mapping](specs/knn-vector-mapping/SPEC.md), [knn-vector-index](specs/knn-vector-index/SPEC.md), [knn-query](specs/knn-query/SPEC.md), [knn-derived-source](specs/knn-derived-source/SPEC.md) | Not served: `/_plugins/_knn/*` (stats, warmup, clear cache, models, train), the Faiss and NMSLIB engines (`method_parameters`, quantization, `rescore`, `expand_nested_docs`, `ignore_unmapped` refused) |
| Neural search | Not served | ML Commons spec lists it as out of its scope | `neural` and `neural_sparse` queries, `text_embedding`, `sparse_encoding` and similar ingest processors, the rerank and normalisation search processors, `hybrid` query. Needs ML Commons models first |
| Notifications | Not served | Alerting spec takes channel ids as stored documents | `/_plugins/_notifications/configs`, `features`, `channels`, `send` and the sinks (webhook, Slack, Chime, email, SNS). Alerting and ISM both call it |
| SQL and PPL | Not served | no code | `/_plugins/_sql`, `/_plugins/_ppl`, `_sql/_explain`, `_sql/close`, JDBC, CSV, raw and Viz formats; a parser, planner and executor over the reader. Large |
| Observability | Not served | no code | `/_plugins/_observability/object`, notebooks, saved queries and visualisations, mostly stored documents; needs PPL for its queries |
| Reports (Dashboards reporting) | Not served | no code | `/_plugins/_reports/*` definitions, instances, on-demand and scheduled; report rendering needs a headless browser, a native-tier question |
| Geospatial | Partly native | `crates/doc/src/mapping/types.rs`, `reader/src/dsl/registered.rs` (geo fields and queries) | Check the mapped geo types against the stock set (`geo_shape` queries, aggregations); the plugin's `ip2geo` datasource and processor and the GeoJSON upload API are not served |
| Asynchronous search | Not served | no code | `/_plugins/_asynchronous_search` submit, get, delete, stats; results held under a task id ([ADR 0072](adr/0072-tasks-are-node-local-memory-with-no-blob-requests.md)) with the retention bound; `wait_for_completion`, `keep_alive`, `keep_on_completion` |
| Cross-cluster replication | Not served on the Rust node | The Java plan lives in `huyz0/cross-cluster-replication` | Native shape is not designed: the node log and blob copy replace translog pull; `/_plugins/_replication/*` start, stop, pause, resume, status, autofollow. Needs an ADR first |
| Job Scheduler | No Rust counterpart | alerting slice 5 builds `oss-meta::schedule` | A shared native scheduler (the extraction is anomaly detection's slice 4) rather than the plugin |
| Security Analytics, Query Insights, Flow Framework, Search Relevance, Rollups, Transforms, Index management UI, Snapshot management | Not served | no code | Each is an unscoped port. Rollups, Transforms and Snapshot Management are Index Management plugin features, so they sit with ISM's follow-ups. The rest need a decision on whether the serverless scope wants them |
| Prometheus exporter | Native | [prometheus-metrics](specs/prometheus-metrics/SPEC.md), `/_prometheus/metrics` | Done for S3 requests, pools, search gate; the stock exporter's metric set is not matched one for one |
| Painless, mustache, analysis and other modules | Native | [ADR 0069](adr/0069-painless-source-compiles-in-rust-with-java-classes-as-functions.md), `crates/ext`, `ext-wasm` | Painless: confirm the front end covers what alerting and ML scripts use |
| Plugin system (WASM, gRPC, native library tiers) | In progress, slices 1 and 2 merged | [plugin-system](specs/plugin-system/SPEC.md), [ADR 0080](adr/0080-one-plugin-system-three-tiers.md) | The intended home of later ports; no Java tier exists or is planned |

## Order that removes Java

1. Each in-flight port lands and removes its prefix from `PLUGIN_PREFIXES` (ISM, alerting, anomaly detection, ML Commons).
2. The last removal deletes the proxy, its settings, its tests and ADR 0046's standing, and edits the docs listed above.
3. Notifications comes next because alerting and ISM both send through it.
4. The unserved families follow in the order the owner picks.
5. Separate decision: keep the mixed-fleet interop surface (wire, formats, push links), or retire it with the last Java node. The Java test oracles and comparison rigs stay either way.
