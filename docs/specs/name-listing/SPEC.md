# Listing index names

## Serves

The plan's metadata-writes milestone, third part: the one enumeration of a deployment's names, a bounded page at a time, and the prefix lookup that answers an index pattern such as `logs-*`. Follows [descriptor-store](../descriptor-store/SPEC.md) and [tombstone-sweep](../tombstone-sweep/SPEC.md). Invariant I-1 (every store call metered) and the S3 rule (every LIST bounded and carrying `// LIST:`): both costs here are set by the page or the cap, never by how many indexes the deployment holds.

## Change

Decision record: [ADR 0008](../../adr/0008-metadata-crate.md) (Accepted).

Ported from huyz0/OpenSearch `serverless/shell/src/main/java/org/opensearch/serverless/metadata/` at `3920dc36`: `DescriptorListing.java` (`listPage`, `namesWithPrefix`), `DescriptorPage.java`, `Names.validatePrefix` and `TooManyMatchesException`.

- `validate_prefix` in `names`: the Java's rules and messages for an index pattern's prefix (empty allowed; at most 255 bytes; no file-name character, `#`, `:`, `[`, `]`, `{`, `}` or control character). Rust strings hold no unpaired surrogate, so that rule has nothing to check.
- `DescriptorStore::list_page(prefix, after, limit, buf, visit)`: the indexes whose names start with `prefix` and sort after `after`, in name order, at most `limit`, each record handed to `visit` borrowing `buf`. Tombstones and aliases take no row, so a page can be short. It answers where to resume, or none when the walk is complete. Each LIST asks for 1,000 keys and the page reads them in order until it is full, resuming after the last one read, so tombstones cost reads but never a LIST each: the LISTs are `ceil(keys read / 1000)`. The Java asks for what the page can still take plus one, which costs a LIST per short refill when tombstones crowd a page. A tombstone is recognised by its prefix, not parsed, as the Java's `readLive`. Reads run one after another; the Java's pluggable fan-out is not ported.
- `DescriptorStore::names_with_prefix(prefix, cap)`: up to `cap` names under `prefix`, in order, unread. Only when more than `cap` keys match are they read, to count live ones (an index or an alias) and drop tombstones; past `cap` live names it refuses with `MetaError::TooManyMatches` and the Java's message. The reads use a tombstone-sized buffer: a register too long for it is live without being fetched whole. Unlike the Java, the names the first walk already listed are read rather than listed again.
- `MetaError::InvalidArgument` for a `limit` or `cap` of 0, sent before any request.
- Not in this change: `listAll` (whole-population, offline only in the Java), parallel reads, alias records.

## Acceptance criteria

- AC-1: on each backend, `list_page` over indexes, tombstones and aliases walks every index exactly once, in name order, across pages of each limit from 1 to past the population, never yields a tombstone or an alias, honours `prefix` and `after`, and answers none at the end.
- AC-2: requests, counted on each backend: a page that reads `n` keys is `ceil(n / 1000)` `List` (at least 1) and `n` `Get`, tombstones among them included; a page that fills with names left is 1 `List`; a page of 1 past 1,000 tombstones is 2 `List`; a page of 1,000 over 1,001 indexes is 1 `List`, and its next page 1 `List`.
- AC-3: `names_with_prefix` answers the names under the prefix with no read when at most `cap` keys match (1 `List` up to 1,000 names); counts only live names when more match, refusing past `cap` with the Java's message; and a refused prefix reads at most `cap + 1` live names plus the tombstones before them.
- AC-4: `validate_prefix` refuses each rule with the Java's message and accepts the empty prefix; `list_page` and `names_with_prefix` refuse an invalid prefix, a limit or cap of 0, sending nothing.
- AC-5: a malformed index record on a page fails the page with `MetaError::Format`, as the Java's: unreadable is not absent. A malformed tombstone is still a tombstone, skipped; a register longer than the caller's buffer fails with `MetaError::Blob`.

## Test plan

- AC-1 to AC-3, AC-5: `crates/meta/tests/descriptors/listing.rs` over the three backends, with exact meter counts. Seen failing against a planted page that does not resume from the last name read, and one that takes a name past its limit.
- AC-4: unit tests in `crates/meta/src/names.rs` and `listing.rs`.

## S3 request budget

| Operation | PUT-class | GET-class | LIST | DELETE | Depth |
|---|---|---|---|---|---|
| `list_page`, n keys read | 0 | n | ceil(n / 1000), at least 1 | 0 | n + LIST |
| `names_with_prefix`, at most `cap` match | 0 | 0 | ceil(cap / 1000), at least 1 | 0 | LIST |
| `names_with_prefix`, more than `cap` match | 0 | live up to `cap + 1`, plus tombstones among them | as above, plus the rest of the walk | 0 | reads + LIST |

LIST is bounded by the page or the cap. Neither scales with the deployment's indexes: a deployment of a hundred million names pays what one of ten does for the same page.

## Switch

None: a new read-only operation with no default behaviour to change.

## Risks

- Not a snapshot: a name created or deleted behind the cursor is missed, ahead of it seen or not; every index that exists for the whole walk appears once. As the Java.
- Reads are serial: a page of 1,000 is 1,000 round trips. The fan-out comes with the REST layer that needs it.
