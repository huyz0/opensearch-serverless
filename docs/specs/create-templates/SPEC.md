# The node's create applies index templates

## Serves

The index-templates part of the snapshots, templates, ingest and auth milestone: `PUT /{index}` on the node inheriting from the matching index template. Builds on [doc-sets](../doc-sets/SPEC.md), [doc-set-cache](../doc-set-cache/SPEC.md), [template-resolution](../template-resolution/SPEC.md) and [create-inherit](../create-inherit/SPEC.md).

## Change

`oss_node::index_templates::TemplateSets` reads the index and component template sets through a node-local `DocSetCache`, resolves the winner for the new index's name and layers it under the request. `IndexService::with_templates` turns it on; `Config::index_templates` (default off) wires it at assembly.

- A template's inherited shard count and mappings apply to the created descriptor.
- A template that carries settings, or composes a component that does, is refused with 501 `unsupported_index_option` rather than dropped, as the node cannot carry settings into the descriptor yet. A request that carries settings or aliases is refused as before.
- An unreadable template, or a winner naming a missing component, is a 400 `invalid_index_template_exception`.

## Acceptance criteria

- AC-1: With templates on, a create inherits mappings from the winning template and its components; a name no template matches is created as before. Test: `a_create_inherits_the_mapping_and_shards_of_the_matching_template`.
- AC-2: A template carrying settings is refused and writes no descriptor. Test: `a_template_that_carries_settings_is_refused_not_dropped`.
- AC-3: With no template stored a create makes one GET (the index template marker) beside its own write, and no LIST. Test: `a_create_with_no_template_stored_reads_only_the_marker`.
- AC-4: With templates off a create makes exactly the requests it made before. Test: `a_create_is_one_conditional_create_and_a_head_one_read`.

## S3 request budget

On: one marker GET when nothing is stored; the component marker is read only when an index template is stored; a changed set re-reads only the documents that moved, a cached one none. Off: unchanged. No LIST.

## Switch

`Config::index_templates`, default off: the cost is one GET on each create. Turning it on by default waits for a measurement of that cost.

## Risks

Scratch buffers are `pit_buffer_bytes` each (two a create, reserved from the body pool); a template set larger than that is refused as a failed create.
