# A refresh makes a large unpublished tail searchable at once

## Serves

Freshness: a refresh makes every acknowledged write searchable. Found by the large-document soak (a count after the first refresh of a 96 MiB bulk of 4 MiB documents returned 22 of 24). Builds on [node-refresh-wait](../node-refresh-wait/SPEC.md) and [node-boot-tail](../node-boot-tail/SPEC.md).

## Change

The cause was not the refresh wait. A refresh the wait confirms publishes nothing, so a reader finds the unpublished tail by asking the writer for it, and a reader whose fetch gets no answer in `FETCH_TIMEOUT` (500 ms) treats the writer as dead and serves its commit. The writer's answer of a few MiB took 650 to 730 ms to write: a wire string was encoded one UTF-16 unit at a time, two passes and a bounds-checked push a byte. The fetch timed out although the writer was alive, and the search answered from a commit that lacked the tail.

- `Output::string` writes a string of ASCII as its count and its bytes in one copy, the bytes the unit loop wrote.
- `Input::string` reads a run of ASCII chars as one slice, with no unit decoding.
- A string with any other char takes the loops as before.

Not changed: `FETCH_TIMEOUT`, the dead-writer rule, the wire format.

## Acceptance criteria

- AC-1: a string round-trips on the same bytes as before, ASCII or not, and an ASCII run cut short is refused.
- AC-2: the first `_count` after a `_refresh` that follows a bulk of five 3 MiB documents counts all five.

## Test plan

AC-2, `crates/node/tests/refresh_large_tail.rs`, was red six runs of six before the change (a count of 3, the commit alone) and green six of six after. AC-1, `crates/transport/tests/wire/stream.rs`, guards the fast path's bytes; the recorded Java rows there cover the rest.

## S3 request budget

Unchanged: no store request is added or removed; the change is in how bytes are encoded in memory.

## Switch

None: it removes work and changes no behaviour, so there is no downside to switch off (a smaller write time for every tail answer, push and forwarded request).

## Risks

A fetch answer that is still slow (a tail of non-ASCII text near the 16 MiB per-shard keep bound on a slow link) can still outlast the timeout; a reader then serves its commit as it does for any silent writer. Also seen, not fixed here: a reader open on a commit older than a lazy publish that finished since can answer `shard_behind_exception` (503) once to a count made straight after the refresh, and the next count succeeds.
