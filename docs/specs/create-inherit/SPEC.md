# Create inherits from its template

## Serves

The index-templates part of the snapshots, templates, ingest and auth milestone: what a create-index request carries once the winning template is layered under it. Follows [template-resolution](../template-resolution/SPEC.md), whose `Inherited` is its input. No store call.

## Change

Ported from huyz0/OpenSearch (`feature/serverlessshell`): `indexadmin/IndexCreateRequest.java` (`shardsWith`, `settingsWith`, `mappingWith`, `normalised`) and `IndexCreation.normalizeMapping`.

`oss_coordinator::indices::create_inherit::inherit(request, inherited)`:

- A request no template matched is returned as it is.
- **Shards:** the template's `index.number_of_shards` stands unless the request named a count in its body or its query. A count the create would refuse (outside `1..=4096`) is refused with the create's own `invalid_shard_count`, where the Java would carry it on unchecked; a value that is not a number leaves the request's own count, as in the Java.
- **Settings:** the template's, flattened under core's `index.` prefix, sit under the request's, which replace a key of the same name (a `null` included). The shard and replica counts are removed from the template's: replicas in a template are dropped, as in the Java, where a request asking for them is refused.
- **Mappings:** the template's merge under the request's by the template-merge rule (a field definition replaces rather than merging parameter by parameter; objects merge). The request's mapping is first unwrapped from `_doc` as the Java does, and a mapping that is not one object, or a `_doc` beside other members, is `malformed_body`. When a template matched but carries no mappings, the request's own is returned normalized.

### Not in this change

The node's create path calling it, and the setting that turns template application on. Today a create that carries settings is refused by the node, so a template's other settings can only apply once the node carries settings into the descriptor.

## Acceptance criteria

- AC-1: A request with no matching template is unchanged. Test: `nothing_inherited_changes_nothing`.
- AC-2: The shard count comes from the template unless the body or query named one; a count the create would refuse is refused; one that is not a number is ignored. Tests: `a_template_supplies_the_shard_count_unless_the_request_names_one`, `a_template_shard_count_the_create_would_refuse_is_refused`.
- AC-3: Template settings sit under the request's, flattened and `index.`-prefixed, structural counts dropped, a request's `null` kept. Test: `template_settings_sit_under_the_requests_and_structural_ones_are_dropped`.
- AC-4: Template mappings sit under the request's with a replacing field rule; a bare or `_doc`-wrapped mapping merges the same; one that is not one object is refused; aliases pass through; mappings stay as given when the template has none. Tests: `template_mappings_sit_under_the_requests_and_a_field_replaces`, `a_mapping_that_is_not_one_object_is_refused`, `a_request_without_mappings_inherits_them_whole_and_aliases_pass_through`.

## Test plan

`crates/coordinator/tests/it/index_create_inherit.rs`, over `templates::resolve`. As for template resolution, the expectations come from reading the Java.

## S3 request budget

Unchanged: no store call.

## Switch

None needed: nothing calls it. The node wiring gets the setting.

## Risks

- Re-serializing a mapping to compact JSON changes its text (not its meaning) when a template matched; a request mapping with duplicate keys is refused as the strict parser refuses it.
