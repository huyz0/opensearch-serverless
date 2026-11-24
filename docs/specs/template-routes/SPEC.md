# Template REST routes

## Serves

The REST handlers part of the snapshots, templates, ingest and auth milestone: `PUT|POST|GET|HEAD|DELETE /_index_template/{name}`, `GET /_index_template`, and the same for `_component_template`, over the document sets. Builds on [doc-sets](../doc-sets/SPEC.md), [template-admin](../template-admin/SPEC.md) and [create-templates](../create-templates/SPEC.md).

## Change

Ported from huyz0/OpenSearch (`feature/serverlessshell`): `rest/TemplateHandler.java`. `oss_node::template_route::TemplateService`:

- A put or post checks the name and the body (`template_admin`), then stores it with `DocSet::put`; `create=true` refuses an existing name (400). A delete removes one name (404 `template_missing` when absent).
- A get reads the whole set as one cached snapshot (`DocSetCache`) and selects all, a trailing-`*` prefix or one name; `HEAD` answers 200 or 404.
- Query parameters core accepts for a cluster manager (`timeout`, `master_timeout`, `cluster_manager_timeout`, `cause`, `flat_settings`, `local`) are accepted and ignored.
- The routes are registered when `Config::index_templates` is on, the same setting that makes a create inherit.

Differences from the Java: the definition is stored as sent, and the Java's action-filter gate is not here (there is no action gate in this node).

## Acceptance criteria

- AC-1: A template is stored, read by name, by prefix and as a list, and removed; the two sets are separate. Tests: `a_template_is_stored_read_listed_and_removed`, `the_two_sets_are_separate`.
- AC-2: What cannot be honoured is refused and stores nothing. Test: `what_cannot_be_honoured_is_refused_and_stores_nothing`.
- AC-3: `create=true` refuses an existing name. Test: `create_true_refuses_a_name_that_exists`.
- AC-4: A read of an unchanged set is one GET and never a LIST. Test: `a_read_of_an_unchanged_set_is_one_get_and_nothing_is_listed`.
- AC-5: The routes exist only with templates on, and a create then inherits what was stored. Test: `the_template_routes_are_served_only_when_templates_are_on`.

## S3 request budget

A put: the marker GET, the content PUT, the marker CAS, and a delete of the replaced content (the doc-set budget). A get: one marker GET when unchanged, plus one GET for each moved document. No LIST. Off: no route, no request.

## Switch

`Config::index_templates`, default off (see create-templates).

## Risks

Each request holds scratch buffers of `pit_buffer_bytes`; a template set that does not fit is refused as a failed request.
