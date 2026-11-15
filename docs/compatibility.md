# OpenSearch compatibility

The goal is real compatibility with OpenSearch, scoped to the serverless capability: a client that works against an OpenSearch serverless deployment works against this one, for every endpoint and parameter in scope.

## Scope

- **In scope:** the REST endpoints, query DSL, mappings and response shapes a serverless search and indexing deployment serves: index lifecycle, bulk and document APIs, `_search` and its DSL, aggregations, mappings and settings that apply to serverless indices.
- **Out of scope:** cluster administration a serverless deployment does not expose (node, shard allocation and snapshot management APIs), and plugins not yet ported.
- Each endpoint and parameter is listed in a compatibility matrix with one of: **matches**, **differs** (with the difference and why), or **refused** (with the error returned). Unknown parameters fail explicitly; they are never ignored silently.

## How compatibility is tested

- **Golden fixtures** recorded from stock OpenSearch at the version the Java shell builds on (3.10.0, built from upstream's `3.10` branch until it is released): requests, the recorded responses, and normalisation rules for fields that legitimately vary (took, generated ids and uuids). They live in `fixtures/compat/<suite>/`; the format and the recording procedure are in [fixtures/compat/README.md](../fixtures/compat/README.md).
- The harness is `oss-compat`: a bounded loader, the normaliser, a comparator that reports each difference with its JSON pointer and both values, and a runner that replays fixtures against anything implementing its `Target` trait.
- The **compatibility suite** runs every fixture against `oss-node` on the in-process S3 emulator and compares the normalised responses. It is a core check: part of the gate once the first REST endpoint exists, and a stage gate in the [roadmap](roadmap.md). Until then the fixtures check themselves: they load, every rule matches its recording, and they replay cleanly against their own recordings with every normalised field changed.
- OpenSearch's own REST YAML suites for in-scope APIs are run the same way, with skips listed and justified.
- Recording fixtures needs a running OpenSearch, so it is a separate, occasional step. Checking them needs nothing but this repository.

Invariant I-13 in [invariants.md](invariants.md) holds the suite to this: `crates/compat/tests/node_search.rs` replays every `_search` step of `fixtures/compat/search/` against a reader node over the shell's blobs of the same indices ([fixtures/compat-blobs/](../fixtures/compat-blobs/README.md)).

## `_search` on a reader node

**Matches** (the replay, and the reader's unit tests against the Java rules): `match_all`, `term`, `match` (standard analyzer, `operator`, `minimum_should_match`, `zero_terms_query`, `boost`), `bool` (`must`, `should`, `filter`, `must_not`, `minimum_should_match`, `adjust_pure_negative`, `boost`) and `range` on integer, long and date fields (`gt`, `gte`, `lt`, `lte`, `from`/`to` with `include_lower`/`include_upper`, `boost`); `size` and `from` in the body or as parameters; `sort` by `_score`, `_doc` or a keyword, integer, long or date field, with missing values last; the response shape, `_source` as stored, `_id` as `Uid.decodeId` gives it, scores as Java prints floats; the error bodies for unknown queries, keys and parameters, malformed JSON, a result window over 10,000, an unmapped sort field and a sort on a text field.

**Refused** (400, `illegal_argument_exception`, "... is not supported by this node yet"): every other query, every other search key OpenSearch knows (aggregations, highlighting, `track_total_hits` and the rest), every other URL parameter, date math in a range, a range's `format`, `time_zone` and `relation`, any query's `_name`, a range on a string field, a search or sort on a field type the reader does not read yet (`geo_point` and others) or without doc values, and an index of more than one shard.

**Differs:**

| What | OpenSearch | This node | Why |
|---|---|---|---|
| Content after the body's object (`{"size":1} {}`) | `parsing_exception` "Unexpected token [START_OBJECT] found after the main object." | `json_parse_exception` "Unexpected character ... after the root value" | the streaming tokenizer stops at the first root value |
| A `term` or `match` value longer than the 1 MiB a body's recording holds | searched | 400 | the recording is bounded (ADR 0004) |
| A keyword-sorted window larger than the request-body pool holds at 32 KiB a value (the longest term) | answered | 429 `circuit_breaking_exception` | hits are reserved before the search runs (ADR 0004) |
| A newer commit of an open index | seen at the next refresh | seen only once the node is told of it | the node never polls the object store for an index (I-4) |
| `took` | the shard's time | the node's time, parse to last hit chosen | normalised in the fixtures |

