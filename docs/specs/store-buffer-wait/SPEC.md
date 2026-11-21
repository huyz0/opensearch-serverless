# A store call waits for an I/O buffer

## Serves

Parity plan R7, issue #217: a search over several shards failed shards with `no I/O buffer free`. Invariant: bounded memory (ADR 0004).

## Change

`PooledStore` can wait up to a bound for a buffer before answering `Busy` (`with_wait`); `BufferPool::checkout_within` is the primitive. The node sets the bound from `Config::store_buffer_wait`. A search opens up to 8 shards at once and each open prefetches up to 16 ranges, so the 16 buffers of a node's pool were outnumbered by readers that each hold one only for the length of one call: they queue now. The pool's size and the memory it reserves do not change.

## Acceptance criteria

- AC-1: a call on a pool with every buffer out, with a wait, succeeds when a buffer comes back inside the wait.
- AC-2: with the wait over, it answers `Busy` and sent no request.
- AC-3: with no wait (the default of `PooledStore::new`) it answers `Busy` at once, as before.
- AC-4: the three-node suite's search from each node finds every document.

## Test plan

`crates/blob/src/pooled.rs` tests (AC-1 to AC-3); `crates/mem` tests for `checkout_within`; AC-4 is `a_search_from_any_node_finds_every_document`.

## S3 request budget

Unchanged: no request is added; a waiting call is sent once it has a buffer.

## Switch

`Config::store_buffer_wait`, default 5 s; zero restores fail-fast. On because a refused shard fails the user's search for no gain, and a wait is bounded and deadlock-free (a holder of a buffer never waits for a second).

## Risks

Under sustained overload a call waits up to the bound before failing instead of failing at once; a request already queued behind a slow store takes longer to answer 429.
