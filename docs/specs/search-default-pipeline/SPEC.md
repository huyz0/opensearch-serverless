# Index default search pipeline

## Serves

R14.6, after [search-pipelines](../search-pipelines/SPEC.md): `index.search.default_pipeline`, the pipeline a search of an index runs when it names none. Stock OpenSearch applies it as the Java shell does not (the shell refuses the setting, see [coordinator-settings-update](../coordinator-settings-update/SPEC.md) AC-4); this node applies it when search pipelines are on.

## Change

- **Setting.** A create and a settings update accept `index.search.default_pipeline` with a non-empty id, or `_none` (no default); a null on update removes it. With `Config::search_pipelines` off the key is refused 501 `unsupported_index_option` on create and 501 `unsupported_setting` on update, as an update refuses it today, so a stored value is never one this node cannot run. The existence of the named pipeline is not checked when the setting is written (OpenSearch does not either).
- **Running.** A `_search` of one named index that has no `?search_pipeline=` runs the pipeline its settings name, exactly as if the request had named it. Any `?search_pipeline=`, including `_none`, replaces the default. A default naming a pipeline that is not stored is 400 `pipeline_missing`, as a named one is. The default is read from the settings the index's shard was opened with (no request: the open is the one the search makes), and only when search pipelines are on.
- **When it takes effect.** A reader takes the setting when it opens the shard, as `index.max_result_window` does ([compatibility](../../compatibility.md)): a reader already holding the index open keeps the old default until it next opens it.
- **Not changed.** A search of a point in time, `_search` without an index, and `_search` without an index take no default (they have no index settings at hand); `_msearch` items ([search-pipeline-body](../search-pipeline-body/SPEC.md)) and `_count` ([count-default-pipeline](../count-default-pipeline/SPEC.md)) run it.
- [coordinator-settings-update](../coordinator-settings-update/SPEC.md) AC-4's refusal now holds only when the node's registry classifies the key as core and refused (search pipelines off); on, the registry owns it.

## Acceptance criteria

- AC-1: On, a create accepts the key with an id and with `_none`, and refuses an empty value 400; off, a create naming it is 501.
- AC-2: On, a settings update sets, replaces and removes it; off, an update naming it is 501 with the words it has today.
- AC-3: A search of an index whose default is a pipeline that cuts hits runs it: the hits are cut. The same search with `?search_pipeline=_none` is not.
- AC-4: A `?search_pipeline=<other>` overrides the default.
- AC-5: A default naming an absent pipeline is 400 `pipeline_missing`.
- AC-6: With search pipelines on, a warm search of an index that has no default makes no request.
- AC-7: A search of an index whose default is `_none` runs nothing.
- AC-8: With search pipelines off, an index holding a default is searched as if it had none, with no extra request.
- AC-9: A search naming a point in time and an index is refused as before, before any default is read.

## S3 request budget

Unchanged for a search without a default (AC-6, row `search.warm.requests`). A search that runs a default adds what a named pipeline adds: one marker GET on a warm node.

## Switch

`Config::search_pipelines`, the switch of [search-pipelines](../search-pipelines/SPEC.md); no new one.

## Risks

An index created with a default and then served by a node with the switch off ignores it silently: the setting is refused on write but a node can be restarted with the switch off. Reading the key at search only when the switch is on keeps that node's searches unchanged; the value stays in the descriptor.
