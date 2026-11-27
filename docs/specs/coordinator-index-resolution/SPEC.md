# The coordinator turns the names in a request into the indices to fan out over

## Serves

Parity plan R7.1, request routing, first piece: the Java shell's `SearchIndexResolver` and `IndexResolution` (rest package, shell 429b4cbc), without the digest-rollup pruning, which is its own piece. Names resolve through `oss-meta`'s routing cache and prefix listing.

## Change

`oss-coordinator` gains `route::resolve`: `resolve_indices(names, request, source) -> Resolution`, where `source` is a trait over what the metadata plane gives (`resolve(name)`: an index with its shard count, an alias with its indices, or nothing; `names_with_prefix(prefix, cap)`; `describe(index)`; `is_system_index(name)`; the pattern cap) and `request` carries `ignore_unavailable` and whether the caller is on the request path. The answer is each index with its shard count in request order, the names `ignore_unavailable` dropped, or a refusal with a status, an error type and a reason, worded as the shell words them.

## Acceptance criteria

- AC-1: a comma list resolves to its indices in request order, once each; an alias expands to the indices it names, in the alias's order, and an index named twice or both directly and through an alias appears once.
- AC-2: an empty name is refused 400 `unsupported_search`; `?` or a star anywhere but the end is refused 501 `unsupported_search`; a bare `*` and a trailing-star prefix are patterns.
- AC-3: a prefix pattern resolves to the names `names_with_prefix` gives, up to the cap; more than the cap is refused 400 `too_many_indices`; a name the listing gives that the register no longer describes is a non-match, not a missing index.
- AC-4: a typed name that does not exist is refused 404 `index_not_found` ("no such index: n"), or skipped and reported when `ignore_unavailable`; a pattern that matches nothing is refused 404 ("no index matches [p]"), or passes when `ignore_unavailable`; if nothing resolves at all it is refused 404 whatever the flag.
- AC-5: an alias naming an index that is gone is refused 404 ("alias [a] names [i], which does not exist"), or skips that index when `ignore_unavailable`.
- AC-6: on the request path a system index the caller typed (or reached through an alias) is refused 403 `system_index`; one a pattern matched is left out, and a pattern that matches only system indices answers as one that matched nothing, without naming them. Off the request path none of this applies.

## Test plan

`crates/coordinator/tests/it/resolve.rs` over an in-memory `source`; one test for each criterion, the refusals' wording asserted exactly.

## S3 request budget

The resolver adds none of its own: the cost is the source's: one routing-cache read for each distinct name, one capped prefix listing for each pattern, and one description for each index an alias names. Requests scale with the names in the request, not the deployment.

## Bounds

At most the pattern cap indices a pattern; the request's names are bounded by the REST layer's line and parameter limits.

## Switch

None: the shell's resolution has no setting and no measured downside.

## Risks

Rollup pruning (the prefix groups' digest registers) is left out and ships as its own piece; until then a wide pattern fans out to every match.
