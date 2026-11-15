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

Invariant I-13 in [invariants.md](invariants.md) tracks this until the suite exists.
