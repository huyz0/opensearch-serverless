# Verified: stored-search-templates

- AC-1: `a_stored_template_searches_what_the_inline_template_does` (crates/node/tests/it/search_template_stored.rs).
- AC-2: `render_takes_the_id_from_the_path_and_the_path_wins`.
- AC-3: `an_msearch_item_names_a_stored_template_and_a_missing_one_fails_alone`.
- AC-4: `a_missing_id_is_404_and_another_language_is_400`, `an_id_resolves_to_mustache_source_and_nothing_else` (crates/node/src/stored_template_source.rs).
- AC-5: `a_stored_template_that_does_not_compile_is_a_script_exception`.
- AC-6: `the_later_of_id_and_source_in_a_body_is_the_template`, `the_later_of_an_id_and_a_source_is_the_template` (crates/reader/src/template/tests.rs).
- AC-7: `a_stored_template_follows_its_script_and_costs_one_marker_get_warm` (crates/node/tests/server/stored_search_template.rs): a put after a read is seen by the next request, a delete is then 404.
- AC-8: `an_unreadable_set_is_503`.
- AC-9: `a_stored_template_follows_its_script_and_costs_one_marker_get_warm`, rows `search_template.stored.requests` and `search_template.inline.requests`; `_render/template` is used so the number is the set's read alone, not a search of an index.

None of these tests was seen red first: they were written with the code and passed on their first run. NOT-RUN: a stored template over a real index through the assembled node (the node tests have no searchable index; `a_stored_template_searches_what_the_inline_template_does` does it over the compat blobs with the source held in memory), the 429 of a request-body pool that cannot hold the read (the pool path is the one pipelines use), a Java node's stored script read here with a recorded stock answer for the 404 and the wrong-language cases (the 404 text is read from `ScriptService`, not recorded), and a reader-only node, which has no stored-script set and refuses an id as before.
