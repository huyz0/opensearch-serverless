# Template admin decisions

## Serves

The REST handlers part of the snapshots, templates, ingest and auth milestone: what `PUT|GET|DELETE /_index_template/{name}` and `/_component_template/{name}` decide, with no store call. The node route over [doc-sets](../doc-sets/SPEC.md) follows in its own change.

## Change

Ported from huyz0/OpenSearch (`feature/serverlessshell`): `rest/TemplateHandler.java` (`put`, `get`, `concreteName`, the response shapes of core's get actions). `oss_coordinator::template_admin`:

- `validate(kind, body)`: the body is one JSON object; an index template names `index_patterns`, a component template does not; `template.aliases` that is not empty is refused 501 `unsupported_template`; `data_stream` must be an object; `template.settings` must be an object and must not ask for replicas above zero (501 `unsupported_setting`).
- `concrete_name(name)`: a mutation names one concrete template; an underscore prefix is 501 `not_implemented` (so `_simulate` is never stored), and a blank or wildcard name is 501.
- `select(docs, name)`: all, a trailing-`*` prefix, or exactly one (404 `template_missing`).
- `render(kind, found)`: the `index_templates` or `component_templates` answer.

Differences from the Java: the definition is stored as sent, not in core's canonical form, and core's setting validation beyond replicas is not applied (a node that cannot carry template settings refuses them at create, see [create-templates](../create-templates/SPEC.md)).

## Acceptance criteria

- AC-1: An index template needs `index_patterns`, a component template must not have them. Test: `an_index_template_needs_patterns_and_a_component_must_not_have_them`.
- AC-2: A body that is not one object is `malformed_body`. Test: `a_body_that_is_not_one_object_is_malformed`.
- AC-3: Aliases, replicas above zero, a non-object `data_stream` and non-object settings are refused. Test: `aliases_replicas_and_bad_shapes_are_refused_not_stored`.
- AC-4: A mutation needs one concrete name. Test: `a_mutation_names_one_concrete_template`.
- AC-5: A get selects all, a prefix or one, and answers under the set's key. Tests: `a_name_selects_one_a_prefix_or_all`, `a_get_answers_the_name_and_the_definition_under_the_sets_key`.

## S3 request budget

None: no store call.

## Switch

None needed: nothing calls it yet.
