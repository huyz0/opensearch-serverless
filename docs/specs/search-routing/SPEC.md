# `routing` and `preference` on `_search` and `_msearch`

## Serves

The `routing` and `preference` gap of [compatibility](../../compatibility.md): a `_search` URL naming either answers 400 (unknown parameter), and an `_msearch` header naming either answers its item as "not supported by this node yet". Clients send both routinely.

## Change

- `routing` (URL on `_search`, header key on `_msearch`) is a comma-separated list of routing values. On an index of one shard it changes nothing. On an index of several shards the search visits only the shards the values route to (`DocumentRouting::shards_for_search`, the function the writes use), so a document written with that routing is found and the other shards are not read. An index with a routing partition, or an empty value, visits every shard.
- `preference` (URL and header key) is accepted and has no effect: a node owns no shard and a shard has no copies, so there is no copy to prefer. Any value is accepted, as OpenSearch accepts any string.
- Routing is a read-path decision made from the descriptor the search already opened: no new request.
- Not here: `routing` on `_count`, `_field_caps` and point-in-time searches; the other `_msearch` header keys.

## Acceptance criteria

- AC-1: on a two-shard index, `?routing=<v>` visits only the shard `<v>` routes to (total 2, hits named `[two][n]` for that shard alone), and a value routing to the other shard visits that one.
- AC-2: `?routing=a,b` visits the shards of both values; with no `routing` every shard is visited.
- AC-3: `?routing=` (empty) visits every shard; on a one-shard index any routing answers what the search without it answers.
- AC-4: `?preference=_local` (or any string) answers what the search without it answers.
- AC-5: an `_msearch` header with `routing` narrows its item as AC-1, and `preference` is accepted.

## S3 request budget

None added: routing only shortens the list of shards a search opens, and the descriptor it reads is the one the search already opened.

## Switch

None: it turns a refusal into an answer and visits fewer shards, never more.

## Tests

`crates/node/tests/it/search_routing.rs`, through HTTP on a two-shard index whose shards hold copies of the same two matching documents, so a total of 4 means both shards were visited and 2 means one.
