# A refresh makes a large unpublished tail searchable at once

## Serves

Freshness: a refresh makes every acknowledged write searchable. Found by the large-document soak (a count after the first refresh of a 96 MiB bulk of 4 MiB documents returned 22 of 24). Builds on [node-refresh-wait](../node-refresh-wait/SPEC.md) and [node-boot-tail](../node-boot-tail/SPEC.md).

## Change

The cause was not the refresh wait. A refresh the wait confirms publishes nothing, so a reader finds the unpublished tail by asking the writer for it, and a reader whose fetch gets no answer in `FETCH_TIMEOUT` (500 ms) treats the writer as dead and serves its commit. The writer's answer of a few MiB took 650 to 730 ms to write: a wire string was encoded one UTF-16 unit at a time, two passes and a bounds-checked push a byte. The fetch timed out although the writer was alive, and the search answered from a commit that lacked the tail.

- `Output::string` writes a string of ASCII as its count and its bytes in one copy, the bytes the unit loop wrote.
- `Input::string` reads a run of ASCII chars as one slice, with no unit decoding.
- A string with any other char takes the loops as before.

Not changed: `FETCH_TIMEOUT`, the dead-writer rule, the wire format.

## Change: a shard owned and not yet published

The first lazy publish of a shard can still be in flight when a refresh and a count come, and a reader that finds the commit register missing for a shard whose head says it was taken answered 404 (it takes that for a lost register). A writer that lets groups of one shard wait together ([write-pipeline](../write-pipeline/SPEC.md)) starts that publish later in a long bulk, so the count after the first refresh failed at depth 8.

- `ShardFiles::open_unpublished_with_meta` opens the empty commit, naming the head's owner as its writer, when the register is missing and the head names a live owner (not past its lease) for this incarnation of the index. Any other head, no head, or a bucketed index is the `NotFound` as before.
- The node's open takes it only when an overlay source and the freshness check are installed, so a search then asks the owner for the tail the empty commit lacks, as for any commit.

## Acceptance criteria


- AC-1: a string round-trips on the same bytes as before, ASCII or not, and an ASCII run cut short is refused.
- AC-2: the first `_count` after a `_refresh` that follows a bulk of five 3 MiB documents counts all five.
- AC-3: the same with `OSS_WRITE_PIPELINE_DEPTH` 8, where the first lazy publish starts late and the reader finds the shard owned with no commit register: it opens the empty commit naming the owner as writer, and asks the owner for the tail. A register missing under any other head is still the lost-register error.

## Test plan

AC-2, `crates/node/tests/refresh_large_tail.rs`, was red six runs of six before the change (a count of 3, the commit alone) and green six of six after. AC-1, `crates/transport/tests/wire/stream.rs`, guards the fast path's bytes; the recorded Java rows there cover the rest.

## S3 request budget

The string change adds or removes no store request. AC-3's change adds none to an open that finds a register; one that does not reads the head (one GET) as it did to decide the register was lost, and the descriptor once more for the empty commit.

## Switch

None: it removes work and changes no behaviour, so there is no downside to switch off (a smaller write time for every tail answer, push and forwarded request).

## Risks

A fetch answer that is still slow (a tail of non-ASCII text near the 16 MiB per-shard keep bound on a slow link) can still outlast the timeout; a reader then serves its commit as it does for any silent writer. Also seen, not fixed here: a reader open on a commit older than a lazy publish that finished since can answer `shard_behind_exception` (503) once to a count made straight after the refresh, and the next count succeeds.
