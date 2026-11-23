# A released point in time closes the shards it holds open

## Serves

R10.6: [node-frozen-search-shards](../node-frozen-search-shards/SPEC.md) keeps each shard a view searches open in the `open_indexes` pool. A release ([node-pit-route](../node-pit-route/SPEC.md)) left them there until they were the least recently searched, holding the pool's charge for a view nobody can search.

## Change

- `OpenSet::release_prefix` takes every open entry whose name starts with a prefix, held or not, and hands them back to drop outside the lock.
- `SearchService::close_view(id)` releases the entries `pit:<id>#` and returns how many it closed. A search running on one finishes on it.
- `PitService::set_release_hook` installs a hook told the id of each view a release removed (not a repeat, not a string that is no id). The node's assembly installs `close_view`.

Only this node's entries close; another node's stay until its pool evicts them. No request is made: closing reads and writes nothing.

## Acceptance criteria

- AC-1: `release_prefix` takes the entries under the prefix, and none whose name only shares its first characters, and the use order forgets them.
- AC-2: `close_view` closes the shards of that view, frees the pool entries, closes none for another id, and a second call closes none.
- AC-3: after a close, a search of a view still held opens the shards afresh.
- AC-4: a release (by path, body or `_all`) tells the hook each id it removed, and not a repeated or malformed one; a second hook is refused.

## Test plan

Unit test in `open_indexes.rs`; `crates/node/tests/it/frozen_search.rs`; `crates/node/tests/pit_route.rs`.

## S3 request budget

None: no request is added or removed.

## Risks

A view released on one node closes its shards only there. A search of a released view is refused at the view's read, so a leftover entry elsewhere is never served.
