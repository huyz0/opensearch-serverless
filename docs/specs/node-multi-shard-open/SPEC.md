# A reader opens any shard of an index

## Serves

Parity plan R7.6, node assembly: the reader opened shard 0 only, and refused an index of more than one shard. A coordinator in the same process searches the shards of an index one by one, so the reader must open, keep and take a pushed commit for each shard. The per-shard search itself, and what a coordinator reduces, are later pieces.

## Change

`oss-node`: `SearchService::open(index, shard)` opens a shard now if it is not open, charged to the `open_indexes` pool; the open set holds a shard each (keyed by the index name for shard 0 and `name#n` for shard n, an index name holding no `#`); the overlay source is asked for, and told of, the shard's own number; `take_push` serves any shard the node has open. The HTTP search keeps its refusal of an index of more than one shard.

## Acceptance criteria

- AC-1: opening shard 1 of a two-shard index costs two GETs (the descriptor and the commit) and holds one open shard; opening shard 0 then holds two; opening either again makes no request.
- AC-2: opening a shard the index does not have is 400 `illegal_argument_exception`, "shard [n] is out of range for index [i]", and opens nothing; an index that does not exist is 404.
- AC-3: a pushed commit reaches the shard it names alone: a push for a shard this node has not open is not taken, one for an open shard is, and one carrying another uuid is not.
- AC-4: the HTTP search of an index of two shards is still refused as before, and every existing single-shard test passes unchanged.

## Test plan

`crates/node/tests/it/shards.rs` over a two-shard variant of a compat index; AC-4 by the existing `an_index_the_node_cannot_serve_is_refused` and the rest of the suite.

## S3 request budget

Two GETs for the first open of any shard, as for shard 0 today; none for a shard already open; no LIST.

## Bounds

One open shard each, charged to the `open_indexes` pool as before; the pool closes the least recently searched when full.

## Switch

None: no setting, no measured downside; a node serves a one-shard index exactly as before.

## Risks

A shard's open is charged like an index's, so an index of many shards fills the pool sooner; that is the pool's purpose.
