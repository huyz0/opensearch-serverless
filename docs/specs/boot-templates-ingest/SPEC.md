# Index templates and ingest pipelines at boot

## Serves

The snapshots, templates, ingest and auth milestone: a booted node can serve the template routes, apply a template at create, serve the pipeline routes and run a pipeline a write names. [create-templates](../create-templates/SPEC.md), [template-routes](../template-routes/SPEC.md), [pipeline-routes](../pipeline-routes/SPEC.md) and [bulk-ingest](../bulk-ingest/SPEC.md) built these behind `Config::index_templates` and `Config::ingest_pipelines`, which only an assembled node's tests could set.

## Change

Two variables read at boot, each a boolean, each off by default:

- `OSS_INDEX_TEMPLATES` (`serverless.index_templates.enabled`): serve `/_index_template` and `/_component_template`, and make a create inherit from the winning template.
- `OSS_INGEST_PIPELINES` (`serverless.ingest.pipelines.enabled`): serve `/_ingest/pipeline`, and run the pipeline a bulk item or a `_doc` write names.

A value other than `true` or `false` refuses to start, as every other switch does. The two stand alone.

## Acceptance criteria

- AC-1: a node started with no variable answers the template and pipeline routes as it did (no such route), and `OSS_INDEX_TEMPLATES=true` serves the template routes while the pipeline routes stay away, `OSS_INGEST_PIPELINES=true` the other way round. Test: `a_booted_node_serves_the_template_and_pipeline_routes_only_when_each_is_set`.
- AC-2: the two read as set, off by default, each settable alone, and a value that is no boolean is refused. Test: `templates_and_pipelines_are_off_until_set_and_each_stands_alone`.

## S3 request budget

Off: no request, as before. On: the budgets of the four specs above (a marker GET on a create with no template stored; a marker GET on a bulk that names a pipeline). No LIST.

## Switch

Both off by default. A template costs one GET on each create, and a pipeline one GET on each bulk that names one; neither is on a read path, but both are requests the old behaviour did not make, so they turn on by default when a measurement of a deployment's create and bulk rate shows the cost negligible, in a change of its own.
