# index.max_result_window per index

## Serves

Parity plan R14.4: the second behaviour-changing setting the settings update route refused 501 ([settings-update-route](../settings-update-route/SPEC.md), after [index-blocks](../index-blocks/SPEC.md)).

## Change

The reader reads `index.max_result_window` from the index descriptor when it opens a shard (`IndexMeta::max_result_window`) and refuses a search whose `from + size` is past it, where it refused only past the node's 10,000. The settings update route accepts the key as a whole number from 1 to 10,000. 10,000 stays the ceiling: a search's hits are held in memory and reserved from the request-body pool by that bound (`hits_bytes`), so a window above it would need its own memory design.

## Acceptance criteria

- AC-1: an index with no `index.max_result_window`, or one that is not a whole number from 1 to 10,000, searches with a window of 10,000.
- AC-2: with the setting at `n`, a search of `from + size` up to `n` passes the window check and one of `n + 1` is refused 400 `illegal_argument_exception`, "Result window is too large, from + size must be less than or equal to: [n] but was [n+1]. See the scroll api ... [index.max_result_window] index level setting."
- AC-3: the node's own limit, 10,000, still applies to an index that sets none.
- AC-4: a value outside 1 to 10,000 that reaches a descriptor (a Java node's, or older) reads as the default, not as a zero window.
- AC-5: `PUT /{index}/_settings` stores a whole number from 1 to 10,000 as the setting the reader reads.
- AC-6: a value above 10,000, a zero, a negative or a non-number is refused 400 `illegal_argument_exception` and nothing is stored.

## Test plan

`crates/reader/src/search_options.rs` and `shard_files.rs` unit tests for AC-1 to AC-4; `crates/node/tests/server/settings_route.rs` for AC-5 and AC-6, including that the route's ceiling is the reader's `MAX_RESULT_WINDOW`.

## S3 request budget

None: the value rides on the descriptor a shard open already reads.

## Bounds

A search's reserved hits stay bounded by 10,000, whatever the setting says.

## Switch

None: a lower window is what the client asked for, and the default is unchanged.

## Risks

The window is read when a reader opens the shard. A reader that already holds the index open keeps the value it opened with until the shard is next opened there (eviction from the open pool, a restart, or a reader that has not opened it); a settings update does not tell readers. Re-reading the descriptor per search would cost a request per search, so it is not done. This is the same as every other setting a reader takes from the descriptor.
