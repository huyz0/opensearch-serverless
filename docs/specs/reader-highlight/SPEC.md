# Reader: highlight

## Serves

Parity task R10.4 (hit features), fifth part; invariant I-13 (REST matches OpenSearch's recordings); ADR 0004 (bounded memory).

## Change

`oss-reader` serves the search body's `highlight` of the unified highlighter, at OpenSearch 3.10.0 from `HighlightBuilder.fromXContent`, `HighlightPhase` and `UnifiedHighlighter` over the engine's port of the Lucene highlighters (`crates/reader/src/fetch/highlight_parse.rs` and `highlight.rs`, wired in `dsl/source.rs`, `search.rs` and `response.rs`; the dependency is `lucene-highlighter` at the revision of the other engine crates).

- Options, each on the highlight or on a field with the field's winning: `pre_tags` and `post_tags` (the first of each), `fragment_size`, `number_of_fragments`, `no_match_size`, `order` (`score` or `none`), `encoder` (`html` or `default`), `require_field_match`, `boundary_scanner` (`sentence` or `word`) and `type: unified`.
- A field entry names a mapped text or keyword field, or a pattern of them. An unmapped name is skipped; a named field of another type is refused.
- Each hit's text is re-analysed from its stored source, the values at the field's path joined by NUL as OpenSearch joins them. A keyword field and `number_of_fragments: 0` give one fragment a value.
- Fragments with only whitespace are dropped; a field with nothing to show is left out; a hit with no field to show carries no `highlight`.
- A hit writes `highlight` after `fields` and before `sort`; its fields come out in `java.util.HashMap` order.
- Unknown keys fail as OpenSearch fails them (`[highlight] unknown field [k]`, with a suggestion), and a parse error inside a field is wrapped by `[highlight] failed to parse field [fields]`.

Not in this change, refused as not supported yet: `fragmenter`, `force_source`, `highlight_query`, `highlight_filter`, `boundary_chars`, `boundary_max_scan`, `boundary_scanner_locale`, `tags_schema`, `matched_fields`, `max_analyzer_offset`, `phrase_limit`, `options`, `fragment_offset`; a type other than `unified`; a value of an option that is not recognised or is negative; `pre_tags` without `post_tags`, or the reverse; an empty tag list. A hit whose source is over 1,000,000 bytes (OpenSearch's `index.highlight.max_analyzed_offset`) and a source that is not an object fail the response; because the response streams, the failure is raised as the hit is written.

## Acceptance criteria

- AC-1: every `_search` step of `fixtures/compat/search/match.json` gets the status and body OpenSearch 3.10.0 gave, replayed against a reader node over the same blobs. The 24 new steps cover a match, tags, the whole value, no match, `no_match_size`, a keyword, a pattern, two fields, `require_field_match: false`, the html encoder, a small `fragment_size`, score order, a field option over the highlight's, the word scanner, a phrase, an unmapped field, no fields, no hit, the order among `fields`, `sort` and `_explanation`, a hit without its source and three parse errors.
- AC-2: the options merge as field, then highlight, then the defaults of OpenSearch.
- AC-3: the keys and values not served are refused with one message, and the unknown and malformed ones fail as OpenSearch fails them.
- AC-4: a plan names the mapped text and keyword fields of each entry, skips what is unmapped, keeps the last options of a field named twice, and refuses another type.
- AC-5: fragments are cut, marked, ordered, and written as OpenSearch does, over values of arrays and paths.
- AC-6: a source too long to re-analyse, or not an object, fails.

## Test plan

- AC-1: the replay (`crates/compat/tests/node_search.rs`).
- AC-2 to AC-6: `fetch/highlight_tests.rs`.

## S3 request budget

A search without `highlight` is unchanged. With it, each hit's source is already read by the fetch; highlighting adds no read. LIST 0, HEAD 0, PUT 0. Idle: 0.

## Memory

Highlighting a hit reserves, once for the response, the bound `scratch_for` gives: the source's token recording and value tree, its text as UTF-16 and the marked-up passages, each at most the text with a pair of tags for every character. The bound is of the stored source's size (at most 1,000,000 bytes), never of the index.
