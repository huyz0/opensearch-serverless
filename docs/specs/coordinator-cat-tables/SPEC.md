# The coordinator renders cat tables and decides the cat and list index requests

## Serves

Parity plan R7.5, cat health and list rendering: the shell's `CatTable` (restcore), the `_cat/health` row of `ClusterHealthHandler`, and the `_list/indices` and `_cat/indices` decisions of `ListIndicesHandler` (shell 429b4cbc) that need no node. The routes, the metadata reads and the channel are the node assembly's.

## Change

`oss-coordinator` gains `cat`: `Table` (the rows of a cat answer, drawn as text or as JSON, as a page of a list walk or not), `unsupported` (the `h` and `s` refusals), `health::row` (the `_cat/health` row), and `indices` (`row`, `plan`, `Paging`, the next-token codec). `route::resolve` shares its prefix-pattern test. Nothing reads the store; a structured format other than JSON is the REST layer's conversion of the JSON drawn here.

## Acceptance criteria

- AC-1: a text table pads each cell but the last of a row to its column's widest cell plus one space, ends each row with a newline, draws the header row only when asked and counts it in the widths only then, and draws a missing cell as `null`.
- AC-2: a structured table is a JSON array of objects whose every field is a string, escaped as the shell's generator escapes; a page is `{"next_token":<token or null>,"<entity>":[..]}`; a text page ends with the line `next_token <token>` (`null` on the last page); `format` absent or `text` draws text and anything else draws structured.
- AC-3: `unsupported` gives the shell's words for `h`, else for `s`, else nothing.
- AC-4: the `_cat/health` row is cluster name, status word, node count twice, and the active and dormant shard counts, those two as `-` when no index was asked about.
- AC-5: an index row is name, uuid, declared shards, `0` replicas, `open` and the mapping version, under the columns `index uuid pri rep status mapping_version`.
- AC-6: `plan` refuses, in this order and in the shell's words: an unsupported cat column option (501); `size` or `next_token` on a `_cat` request (400); a `size` that is not a number or is outside 1 to 5000; a `next_token` on anything but one prefix pattern; a token that does not decode or was issued for another prefix (400, "tainted"). It answers no paging for `_cat`, one page for named indices on `_list`, and a walk (prefix, resume name, size 500 by default) for one prefix pattern.
- AC-7: a token made for a prefix and a name decodes to them; it is URL-safe base64 without padding; padded input decodes too; anything else is refused as tainted.

## Test plan

`crates/coordinator/tests/cat.rs`: one test per criterion, words and layouts asserted exactly.

## S3 request budget

Zero: pure decisions and rendering over what the caller read.

## Bounds

The text and JSON are one allocation each, bounded by the rows the caller gives; a page is at most 5000 rows (`MAX_SIZE`).

## Switch

None: the shell's cat tables have no setting and no measured downside.

## Risks

A column width counts UTF-16 units as Java's `String.length` does, so a name outside the basic plane pads as the shell pads it. A format such as YAML is drawn as JSON here and converted by the edge.
