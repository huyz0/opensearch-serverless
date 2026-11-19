# The coordinator reads a create-index request

## Serves

Parity plan R7.4, index admin REST, first piece: the decisions in the Java shell's `IndexCreateRequest` (indexadmin package, shell 429b4cbc) that need no node: what the body means (nothing, a bare mapping or an envelope of settings, mappings and aliases), which asks are refused, how settings are named and which are consumed. The route, the template layering, the descriptor write and the reply are separate pieces.

## Change

`oss-coordinator` gains `indices::settings` (`flatten`: a settings object as dotted keys with core's `index.` prefix) and `indices::create` (`parse`, `CreateRequest`, `MAX_SHARDS`). `Refusal::new` becomes public so every REST decision refuses in one type.

## Acceptance criteria

- AC-1: no body or an empty one asks for the query's shard count (1 when the query gave none), marks it explicit when it is not 1, and carries no mapping, settings or aliases.
- AC-2: a body that is not a JSON object (malformed, duplicate key, an array, a scalar) is refused 400 `malformed_body`, "the body is not a JSON object".
- AC-3: an object holding none of `settings`, `mappings`, `aliases` is the mapping itself: its text is carried as written, with the query's shard count and nothing else.
- AC-4: in an envelope, `mappings` and `settings` and `aliases` that are present but not objects (null included) are refused 400 `malformed_body`, "'<name>' must be an object"; a mappings object is carried as compact JSON.
- AC-5: settings are flattened to dotted keys (nested objects joined with `.`), a key that does not start with `index.`, end in `*` or start with `archived.` gets the `index.` prefix, values are carried as text (strings as written, numbers as written, booleans as `true`/`false`, arrays as compact JSON, null as no value), later keys replace earlier ones, and the keys come out sorted.
- AC-6: `index.number_of_replicas` other than `0` (in any spelling AC-5 maps to it) is refused 501 `unsupported_setting` in the shell's words; `0` is accepted and consumed.
- AC-7: the shard count is `index.number_of_shards` when given, else the query's; one that is not a number is refused 400 `malformed_body`, "number_of_shards must be a number"; one outside 1 to 4096 (from either source) is refused 400 `invalid_shard_count`, "number_of_shards must be between 1 and 4096, got N"; naming it in settings marks it explicit.
- AC-8: the settings carried on are everything but the shard and replica counts, and none when nothing remains.
- AC-9: each alias keeps its options in order (an alias whose value is not an object has none); `filter`, `routing`, `index_routing`, `search_routing` (any non-null value) and `is_hidden: true` are refused 501 `unsupported_alias_option` in the shell's words, the first alias in order that holds one deciding.

## Test plan

`crates/coordinator/tests/index_create.rs`: one test per criterion; refusal words asserted exactly.

## S3 request budget

Zero: the parse reads nothing.

## Bounds

The tree is bounded by the body, which the HTTP edge bounds and `oss_doc::mapping::json::parse` caps at 8 MiB.

## Switch

None: the shell's create has no setting and no measured downside.

## Risks

Arrays in settings are carried as JSON text where core makes a string list; no create-time setting this deployment reads is a list.
