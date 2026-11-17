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

- **request**: `method` (GET, PUT, POST, DELETE, HEAD), `path` (starting with `/`, no query string), optional `query` (string values), and at most one of `body` (one JSON document) or `ndjson` (an array, sent one document per line, for `_bulk` and `_msearch`).
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
| [mapping/deprecated.json](mapping/deprecated.json) | parameters a type does not declare (`store`, `boost`, `index` and the rest), dropped silently as OpenSearch drops them |
| [search/match-all.json](search/match-all.json) | `_bulk` with explicit ids and `refresh=true`; `_search` with `match_all` |
| [search/term.json](search/term.json) | `term` on keyword, text and integer fields, short and long forms |
| [search/match.json](search/match.json) | `match` with the standard analyzer: `or`, `and`, `minimum_should_match`, numbers |
| [search/bool.json](search/bool.json) | `bool` with `must`, `should`, `filter`, `must_not` and `minimum_should_match`, nested |
| [search/range.json](search/range.json) | `range` on integer and date fields, inclusive and exclusive, with rounding |
| [search/paging-sort.json](search/paging-sort.json) | `size` and `from` in the body and as parameters; `sort` by fields and by score |
| [search/errors.json](search/errors.json) | OpenSearch's error bodies for queries, keys and parameters the subset refuses, and the result window |

The mapping fixtures are replayed through the writer's mapping by `crates/writer/tests/mapping_fixtures.rs`; the 3.10.0-SNAPSHOT they were recorded from was built from huyz0/OpenSearch `429b4cbc` (`./gradlew localDistro`). The search fixtures' indices are also written through the Java shell into [fixtures/compat-blobs/](../compat-blobs/README.md), so a reader node replays their `_search` steps.
