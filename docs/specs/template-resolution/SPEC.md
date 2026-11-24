# Template resolution

## Serves

The index-templates part of the snapshots, templates, ingest and auth milestone: what an index inherits at creation. Stock composable and component templates are how a schema is authored (the schema-sharing decision of 2026-11-12), so create-index and the data-stream and rollover paths need this decision, as the Java's `TemplateResolver` gives them. Follows [doc-sets](../doc-sets/SPEC.md) and [doc-set-cache](../doc-set-cache/SPEC.md), whose snapshot is its input.

Invariants served: I-1 and the request rules by touching no store at all: it is a pure function over documents already read, so an index create's template cost is the snapshot's one GET of the marker.

## Change

Ported from huyz0/OpenSearch (`feature/serverlessshell`): `template/TemplateResolver.java` (`resolve`, `matchingTemplateNames`, `Winner`, `inherit`, `merge`, `mergeMappings`, `mergeProperties`, `deepMerge`, `priority`, `glob`).

In `oss-coordinator`, `templates`:

- `resolve(templates, component, index)` answers `Inherited { settings, mappings, from, data_stream }`. A template matches when any `index_patterns` entry matches the whole name, `*` the only wildcard (a single string stands for a list of one). The highest priority wins; a tie goes to the smaller name in Java string order. The winner's `composed_of` components apply in order, then the template: settings merge object by object, mappings merge object by object with `properties` field by field, and a pair that is not two objects (two object or nested fields, for a property) is replaced by the later.
- Priority is read as core's `Numbers.toLong(text, true)`: an integer, or a decimal truncated toward zero, a string as well as a number; absent or `null` is 0; negative or unreadable is "index template priority must be a nonnegative long". Only a matching template is asked.
- Every template is read, so one that cannot be read is an error whether or not it matches ("stored template [x] could not be read"); a component the winner names that is not stored is "index template [t] composes component template [c], which does not exist", never skipped.
- `matching_template_names(templates, indices)` answers the winner of each distinct index, in first-seen order.
- `Inherited::{is_data_stream, timestamp_field}`: a `data_stream` object makes data streams; the field is `timestamp_field.name`, else `@timestamp`.

**Differs:** a pattern, component name or `timestamp_field.name` that is an array or object is read as its JSON text; the Java reads its map's own `toString`. No stored template holds one.

### Not in this change

The create-index wiring that calls it, the REST handlers that store templates, and validation of a template at PUT.

## Acceptance criteria

- AC-1: No template, or none matching, inherits nothing. Patterns match the whole name with `*` the only wildcard; a single string is a list of one; any entry may match; none or `null` never matches. Tests: `no_template_means_nothing_inherited`, `patterns_match_the_whole_name_with_stars_as_the_only_wildcard`.
- AC-2: The highest priority wins, a tie goes to the smaller name in UTF-16 order whatever the order given, no priority is 0. Test: `the_highest_priority_wins_and_a_tie_goes_to_the_smaller_name`.
- AC-3: A priority is an integer, a decimal truncated toward zero, or the same as a string, and a negative or unreadable one is refused with the Java's message, asked only of a matching template. Test: `a_priority_is_read_as_core_reads_a_long`.
- AC-4: Components apply in order under the template; settings and mappings merge as the Java merges them (objects merge, scalars and arrays replace, `properties` merge field by field and only object or nested fields merge, a member keeps its first position). Tests: `a_matching_template_gives_its_settings_and_mappings`, `components_apply_in_order_and_the_template_on_top`, `a_field_replaces_the_one_before_unless_both_are_objects`, `arrays_and_scalars_are_replaced_not_merged`.
- AC-5: A missing component, and a stored template or component that cannot be read (matching or not), are the Java's errors. Tests: `a_component_that_does_not_exist_is_reported`, `a_stored_template_that_cannot_be_read_is_reported_matching_or_not`.
- AC-6: A data stream template says so and names its timestamp field; the winners of many indices come back once each in first-seen order. Tests: `a_data_stream_template_says_so_and_names_its_timestamp_field`, `many_indices_each_get_their_winning_template_name`.

## Test plan

`crates/coordinator/tests/templates.rs`. ⚠️ The expectations come from reading `TemplateResolver` and core's `Numbers.toLong`, not from recorded Java output: a fixture recorded from a running shell is the open follow-up.

## S3 request budget

Unchanged: no store call. Cost is CPU in the number of templates (at most 1,000) times their patterns, once an index create.

## Switch

None needed: nothing calls it yet. The create-index wiring gets the setting that turns template application on.

## Risks

- Priority parsing is hand-written to match `BigDecimal` truncation; the test table pins its cases, and an odd form it refuses the Java accepts would refuse an index create, loudly.
- Settings keep the Java's nested-object merge, so a flat key (`index.refresh_interval`) and a nested one (`index`: {...}) in two templates do not merge, as in the Java.
