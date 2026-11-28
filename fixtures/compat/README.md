# Compatibility fixtures

Golden fixtures for the compatibility suite ([docs/compatibility.md](../../docs/compatibility.md)), one JSON file per fixture, grouped by suite (`index/`, `search/`). `oss-compat` parses, normalises and replays them; `record.py` fills in their responses from stock OpenSearch.

Requests and normalisation rules are written by hand. **Responses are never written by hand**: they come from `record.py`, and every fixture says which OpenSearch build they came from.

## Format

```json
{
  "name": "match_all over two documents",
  "description": "optional, for people",
  "recorded_from": {"distribution": "opensearch", "version": "3.10.0-SNAPSHOT", "build_hash": "..."},
  "cleanup": [
    {"method": "DELETE", "path": "/compat-match-all", "query": {"ignore_unavailable": "true"}}
  ],
  "steps": [
    {
      "name": "search match_all",
      "request": {"method": "POST", "path": "/compat-match-all/_search", "body": {"query": {"match_all": {}}}},
      "response": {"status": 200, "headers": {"content-type": "application/json; charset=UTF-8"}, "body": {"took": 3}},
      "normalise": [{"pointer": "/body/took", "reason": "elapsed time"}]
    }
  ]
}
```

- **request**: `method` (GET, PUT, POST, DELETE, HEAD, PATCH), `path` (starting with `/`, no query string), optional `query` (string values), and at most one of `body` (one JSON document), `raw` (a string sent as written, for a body that is no single JSON value: duplicate keys, trailing data) or `ndjson` (an array, sent one document per line, for `_bulk` and `_msearch`).
- **response**: `status`, `headers` and `body` as recorded. Only the headers listed are compared; `record.py` keeps the names already listed, or records `content-type` for a new step. `body` is absent when the response had none.
- **normalise**: rules for fields that legitimately vary. `pointer` is a JSON pointer into `{"status", "headers", "body"}`; a `*` segment matches every array element and object member (`/body/hits/hits/*/_id`). `reason` is required. Both the recorded and the actual value are replaced by `"<normalised>"` before comparing.
- **cleanup**: requests sent before the steps (and by the recorder after them), so a fixture runs against a target that is not fresh. Their responses are not compared.
- Unknown keys are refused, so a misspelt key cannot silently drop a rule.

Rules for rules:

- A rule replaces only values that exist. A field the target leaves out is still reported missing; a rule never hides a structural difference.
- A rule that matches nothing in the recording is stale and fails the step, and `record.py` refuses to write it.
- A rule replaces a whole value. When a message embeds a generated value (an index uuid inside an error `reason`), the whole message is normalised, and the rule's reason says so. Keep such rules rare: prefer a request that makes the value deterministic (explicit `_id`s, one shard, `refresh=true`).
- Fixture index names start with `compat-` and are unique per fixture, so fixtures never share state.

## Recording

The version to record from is the one the Java serverless shell builds on: `opensearch` in `gradle/libs.versions.toml` on huyz0/OpenSearch, branch `feature/serverlessshell` (3.10.0 as of 2026-11-14). 3.10.0 is not released yet, so it is built from upstream's `3.10` release branch.

1. Build a stock distribution (about 4 minutes, needs JDK 21 and network access for Gradle):

   ```bash
   git clone --depth 1 --branch 3.10 https://github.com/opensearch-project/OpenSearch os-3.10
   cd os-3.10 && ./gradlew localDistro -Dbuild.docker=false
   ```

   The distribution lands in `build/distribution/local/opensearch-3.10.0-SNAPSHOT`. Once 3.10.0 is released, the release tarball works the same way.

   **Release tarball instead of a build** (3.10.0 is released; about 1.1 GB, keep 15 GB of disk free):

   ```bash
   mkdir -p /home/user/stock && cd /home/user/stock
   curl -sS -O https://artifacts.opensearch.org/releases/bundle/opensearch/3.10.0/opensearch-3.10.0-linux-x64.tar.gz
   curl -sS -O https://artifacts.opensearch.org/releases/bundle/opensearch/3.10.0/opensearch-3.10.0-linux-x64.tar.gz.sha512
   sha512sum opensearch-3.10.0-linux-x64.tar.gz   # must match the .sha512 file
   tar xzf opensearch-3.10.0-linux-x64.tar.gz
   # optional, for a faster start: remove every directory in opensearch-3.10.0/plugins except opensearch-knn
   chown -R 65534:65534 /home/user/stock           # the sandbox is root; OpenSearch refuses root
   cd opensearch-3.10.0 && setpriv --reuid 65534 --regid 65534 --clear-groups env HOME=/home/user/stock \
     OPENSEARCH_JAVA_OPTS="-Xms1g -Xmx1g" nohup bin/opensearch -Ediscovery.type=single-node \
     -Ecluster.routing.allocation.disk.threshold_enabled=false > ../os.log 2>&1 &
   ```

   With the security plugin kept, add `-Eplugins.security.disabled=true` for plain HTTP, or record with `RECORD_AUTH`. Java test vectors compile with `javac -proc:none -cp 'opensearch-3.10.0/lib/*'`.

2. Start it as a non-root user (OpenSearch refuses root), single node, with the disk threshold off so a nearly full sandbox disk does not block index creation:

   ```bash
   bin/opensearch -Ediscovery.type=single-node -Ecluster.routing.allocation.disk.threshold_enabled=false
   ```

   If an earlier run hit the disk threshold, clear the persisted block: `curl -XPUT localhost:9200/_cluster/settings -H 'content-type: application/json' -d '{"persistent":{"cluster.blocks.create_index":null}}'`.

3. Record, from the repository root:

   ```bash
   python3 fixtures/compat/record.py --url http://localhost:9200 fixtures/compat/*/*.json
   ```

   It refuses a server whose version does not start with `--expect-version` (default `3.10.0`).

4. Check that the rules cover everything that varies, by recording a second copy and replaying the first against it:

   ```bash
   cp -r fixtures/compat /tmp/rerecorded
   python3 fixtures/compat/record.py /tmp/rerecorded/*/*.json
   COMPAT_REPLAY_FROM=/tmp/rerecorded cargo test -p oss-compat --test recorded
   ```

   A failure prints each differing field with both values; add a rule with its reason, or make the request deterministic, and record again.

5. Read the diff of the fixtures before committing: a recorded error where a success was meant (a 403 from a disk block, say) is still a recording.

Neither the gate nor CI runs `record.py`; checking fixtures needs nothing but this repository (`cargo test -p oss-compat`).

## Fixtures

| File | Covers |
|---|---|
| [index/create-index.json](index/create-index.json) | `PUT /<index>` with settings and mappings; the second create's `resource_already_exists_exception` |
| [mapping/explicit-types.json](mapping/explicit-types.json) | `PUT /<index>` with every field type the writer indexes, parameters on and off their defaults; `GET _mapping` |
| [mapping/dates.json](mapping/dates.json) | date formats and `print_format`, parameter order with `meta` and `boost`, `norms` and multi-field merges |
| [mapping/root-options.json](mapping/root-options.json) | the root's `dynamic`, `date_detection`, `numeric_detection` and `_routing` |
| [mapping/put-mapping.json](mapping/put-mapping.json) | `PUT _mapping`: added fields and sub-fields, updateable parameters, and the conflict messages |
| [mapping/invalid.json](mapping/invalid.json) | `MapperParsingException` messages for unknown parameters and types, a missing type, bad values, nulls, `meta` limits, and one index per refusal |
| [mapping/dynamic.json](mapping/dynamic.json) | dynamic mapping of every JSON kind, and refused values |
| [mapping/detection.json](mapping/detection.json) | `date_detection` off and `numeric_detection` on |
| [mapping/dynamic-strict.json](mapping/dynamic-strict.json) | `dynamic: strict` and `false`, at the root and in objects |
| [mapping/values.json](mapping/values.json) | how each type reads document values: coercion, ranges, `ignore_malformed`, `null_value`, booleans, `ignore_above`, date formats, objects and arrays, mixed dynamic arrays, bad field names |
| [mapping/document-edges.json](mapping/document-edges.json) | metadata fields in a body, the depth limits, coercion messages, many-key previews, dotted names, the template dynamic modes, strict nulls, object and field conflicts |
| [mapping/deprecated.json](mapping/deprecated.json) | parameters a type does not declare (`store`, `boost`, `index` and the rest), dropped silently as OpenSearch drops them |
| [search/match-all.json](search/match-all.json) | `_bulk` with explicit ids and `refresh=true`; `_search` with `match_all` |
| [search/term.json](search/term.json) | `term` on keyword, text and integer fields, short and long forms |
| [search/match.json](search/match.json) | `match` with the standard analyzer: `or`, `and`, `minimum_should_match`, numbers |
| [search/bool.json](search/bool.json) | `bool` with `must`, `should`, `filter`, `must_not` and `minimum_should_match`, nested |
| [search/range.json](search/range.json) | `range` on integer and date fields, inclusive and exclusive, with rounding |
| [search/paging-sort.json](search/paging-sort.json) | `size` and `from` in the body and as parameters; `sort` by fields and by score |
| [search/errors.json](search/errors.json) | OpenSearch's error bodies for queries, keys and parameters the subset refuses, and the result window |

The mapping fixtures are replayed through the writer's mapping by `crates/writer/tests/mapping_fixtures.rs`; the 3.10.0-SNAPSHOT they were recorded from was built from huyz0/OpenSearch `429b4cbc` (`./gradlew localDistro`). The search fixtures' indices are also written through the Java shell into [fixtures/compat-blobs/](../compat-blobs/README.md), so a reader node replays their `_search` steps.

## Breadth fixtures

[search-breadth/](search-breadth/) records term-level, full-text and compound queries and bucket aggregations from the stock 3.10.0 release tarball (`artifacts.opensearch.org`, security plugin removed) and `node_cluster.rs` replays them whole. [search-breadth-gaps/](search-breadth-gaps/) holds recorded answers for what the node refuses today; nothing replays it against a node, and a step moves to `search-breadth/` when the node serves it.

## Nested fixtures

[nested-search/nested-query.json](nested-search/nested-query.json) and [nested-search/nested-aggs.json](nested-search/nested-aggs.json) cover the `nested` query (bool and range inside it, every `score_mode` with scores, `boost`, inside `bool`, unmapped and non-nested paths, the root searching a nested field's children) and the `nested` and `reverse_nested` aggregations (metrics, `terms`, a query around them, beneath a keyword and a numeric `terms`, and their refusals). Recorded from the stock 3.10.0 build (b9f09249, built from the `3.10.0` tag with `./gradlew localDistro`). The node cannot take them from `compat-blobs`, which holds no nested index, so `an_assembled_node_answers_every_nested_fixture_as_stock_opensearch_did` (`crates/compat/tests/node_cluster.rs`) replays them whole through a node with `serverless.nested.enabled` on. [nested-search/nested-named.json](nested-search/nested-named.json) was recorded from the stock 3.10.0 release tarball (as the k-NN fixtures are) and covers `_name` with `inner_hits` and the `include_named_queries_score` URL parameter and body key. One parse error's column is normalised: the replay sends a body with its keys sorted.

## k-NN fixtures

`mapping/knn-*.json` and `knn/` are recorded from the stock 3.10.0 release tarball (`artifacts.opensearch.org`, SHA-512 checked against the published sum) with its bundled k-NN plugin, which the 3.10 source build above does not include. Start it as in step 2, with the security plugin disabled. The Faiss engine needs its native libraries on the library path, which the tarball ships under `plugins/opensearch-knn/lib`: start the node with `LD_LIBRARY_PATH=<install>/plugins/opensearch-knn/lib` and `-Djava.library.path=<same directory>` in `OPENSEARCH_JAVA_OPTS`, or the node exits when a request reaches Faiss.

## Stock plugin fixtures

Recorded from the same tarball with its bundled plugins, the same way (`record.py`), and replayed by `oss-compat`:

- `knn-options/`: Lucene query options (`ef_search`, `rescore`, `expand_nested_docs`, `min_score`, `max_distance`), the plugin API (stats, warmup, models, `_train`), 80 mapping cases across Faiss, Lucene and nmslib with their validation messages, and Faiss search options.
- `notifications/`: every channel type, list filters, sort and paging, test sends against a local target, refusals.
- `observability/`: saved objects with explicit ids. Lists are search-based, so each list step follows a refresh of `.opensearch-observability`.
- `security-api/`: the REST management API, recorded from a node with the security plugin on (see [../stock-plugins/README.md](../stock-plugins/README.md)).

Plugins that generate ids (alerting, ISM, anomaly detection, ML Commons, `_train`) and async search use the scenario format in [../stock-plugins/README.md](../stock-plugins/README.md) and [../async-search/](../async-search/); SQL and PPL are in [../sql/README.md](../sql/README.md).

## Geo fixtures

`geo/` records the geospatial features of stock OpenSearch 3.10.0 (the release tarball, `opensearch-geospatial` and `opensearch-job-scheduler` kept, the other plugins removed; SHA-512 of the tarball checked), so the native geo fields are compared against stock rather than against a brute-force oracle. Every request is a plain `record.py` step on a fresh single-shard index; documents use explicit ids, and the keys of a body are written sorted so the replay sends what the recorder sent. A step whose message embeds an object hash (`KeywordFieldMapper$KeywordFieldType@...`) is normalised whole.

| File | Covers |
|---|---|
| [geo/point-values.json](geo/point-values.json) | `geo_point` values in every input form (object, `lat,lon`, geohash, `[lon, lat]`, WKT, GeoJSON, arrays, z values, boundaries) and the ones refused with their error bodies; read back through `_source`, `fields` and `docvalue_fields` with their formats, `get`, `mget`, `exists`, and the queries, sorts and aggregations a geo field refuses without a geo feature |
| [geo/point-options.json](geo/point-options.json) | `ignore_malformed` (field and index setting, with `_ignored`), `ignore_z_value`, `null_value`, `doc_values` and `index` off, `store`, `copy_to`, sub-fields, unknown and bad parameters, dynamic mapping and templates, put-mapping updates and conflicts, objects, dotted names and nested |
| [geo/query-distance.json](geo/query-distance.json) | `geo_distance`: point forms, distance units and malformed distances, `distance_type`, `validation_method` and out-of-range points, poles and the date line, `boost`, `_name`, bool nesting, refusals, `_validate/query`, `_explain`, `_count`, `_msearch` |
| [geo/query-bounding-box.json](geo/query-bounding-box.json) | `geo_bounding_box`: corner forms, `wkt` `BBOX`, `top`/`left`/`bottom`/`right`, date-line boxes, poles, `validation_method` and `type`, refusals |
| [geo/query-polygon.json](geo/query-polygon.json) | `geo_polygon`: point forms, open and closed rings, both windings, concave and self-intersecting polygons, date line and poles, `validation_method`, too few points, refusals |
| [geo/sort-geo-distance.json](geo/sort-geo-distance.json) | the `_geo_distance` sort (every `unit`, `mode`, `order` and `distance_type`, several points, missing and multi-valued documents, `search_after`, refusals, 500s from a non-geo field), plus the geo decay functions and the painless geo accessors |
| [geo/aggs-geo.json](geo/aggs-geo.json) | `geo_bounds`, `geo_centroid`, `geohash_grid`, `geotile_grid`, `geohex_grid` and the `geo_distance` aggregation: precisions, `size`, `shard_size`, `bounds`, sub-aggregations, empty and date-line cases, refusals |
