# Stock plugin recordings

Answers of the stock OpenSearch 3.10.0 plugins, recorded from a real node. Java is the oracle here, never the runtime: these files are what the Rust plugin ports are checked against.

| File | Plugin | Recorded by |
|---|---|---|
| `alerting.json` | alerting: monitors, triggers, destinations, alerts, acknowledge | `alerting.py` |
| `ism.json` | index state management: policies, add, remove, explain, change_policy, retry | `ism.py` |
| `ad.json` | anomaly detection: detectors, validate, preview, results, tasks | `ad.py` |
| `ml.json` | ML Commons: models, remote predict, deploy, agents, stats | `ml.py` |
| `ad-results.json` | anomaly detection after historical and real time runs: result documents, `_topAnomalies` (orders, sizes, category fields, every refusal), profile of every type, tasks, category detector preview and validate | `ad_results.py` |
| `knn-train.json` | k-NN `_train`, model routes and states | `knn_train.py` |

The k-NN, notifications, observability and security-API recordings are in `fixtures/compat/` (the `oss-compat` format), async search in `fixtures/async-search/`, SQL and PPL in `fixtures/sql/`.

## Format

These plugins generate ids, so a scenario is a list of steps whose requests and answers use placeholders.

- A step may `capture` a value of its answer under a label (a JSON pointer). Later steps write `{label}` in a path, a query value or any string of a body.
- The recording never holds a generated value: every captured value in an answer becomes `<label>`, keys named volatile (timestamps, `_version`, `_seq_no`, uuids) become `<volatile>`, numbers of nine or more digits and `version|seqNo|term [n]` in messages become `<n>`.
- A step may `poll` until a pointer of its answer holds a value (the plugin works later); only the final answer is kept.
- Requests are written in the scenario. Responses only ever come from the node.

`cmp.py A B` diffs two recordings. A recording is only committed once two runs differ nowhere.

## Recording

1. Get the release tarball (see [fixtures/compat/README.md](../compat/README.md)), keep the plugins you record (`alerting`, `opensearch-anomaly-detection`, `opensearch-index-management`, `opensearch-job-scheduler`, `opensearch-knn`, `opensearch-ml`, `notifications`, `notifications-core`, `opensearch-observability`, `opensearch-sql`, `opensearch-asynchronous-search`, `opensearch-system-templates`), and run as a non-root user with `-Eplugins.security.disabled=true`.
2. For k-NN with Faiss, set `LD_LIBRARY_PATH` and `-Djava.library.path` to `plugins/opensearch-knn/lib`.
3. Webhook and notification scenarios send to `python3 fixtures/stock-plugins/webhook_target.py` (127.0.0.1:18080; `/<status>` answers that status, `/redir` a 302, `/json` a JSON body).
4. Run a recorder: `python3 fixtures/stock-plugins/alerting.py --url http://localhost:9200 /tmp/alerting.json`, twice, compare with `cmp.py`, then copy the result here. ISM takes about eight minutes a run (job interval). `ad_results.py` takes about two minutes and wants a fresh node for each run (a node that already ran it answers its first result searches with 500s); keep only `opensearch-anomaly-detection` and `opensearch-job-scheduler`. Stock 3.10.0 answers `_topAnomalies` with 500 `Fail to search` while the result index's segments are compound files (the script reads `_source`); the recording merges them to plain segments first, as its step names say.

## Security

`fixtures/compat/security-api/` needs a node with the security plugin on. The demo installer (`plugins/opensearch-security/tools/install_demo_configuration.sh -y -i -c`) wants a strong `OPENSEARCH_INITIAL_ADMIN_PASSWORD`, so set one; set `network.host: 127.0.0.1` to skip bootstrap checks, and do not set `discovery.type`. Record with `RECORD_AUTH=admin:<password> RECORD_INSECURE=1` and the node's HTTPS base URL. The recorded fixtures contain only the fixed passwords the scenario creates for its own test users.
